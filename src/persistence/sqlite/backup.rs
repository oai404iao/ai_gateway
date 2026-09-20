//! Offline, checksummed database/spool snapshots. Restore never overwrites an existing tree.

use super::{SqliteDatabase, ownership::DatabaseOwner};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    database_id: uuid::Uuid,
    directories: Vec<PathBuf>,
    files: BTreeMap<PathBuf, Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    bytes: u64,
    sha256: [u8; 32],
}

fn invalid(message: &str) -> Box<dyn std::error::Error> {
    io::Error::other(message).into()
}

fn directory(path: &Path) -> Result<File> {
    if !path.is_absolute() || path.canonicalize()? != path {
        return Err(invalid("backup paths must be absolute and unaliased"));
    }
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
    {
        return Err(invalid(
            "backup directories must be owned and not writable by other users",
        ));
    }
    Ok(file)
}

fn read_file(path: &Path) -> Result<File> {
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || (metadata.mode() & 0o007 != 0
            && !(metadata.len() == 0
                && metadata.mode() & 0o003 == 0
                && matches!(
                    path.file_name().and_then(|n| n.to_str()),
                    Some("writer.lock" | "spool.lock")
                )))
    {
        return Err(invalid(
            "snapshot files must be owned, private, regular and unaliased",
        ));
    }
    Ok(file)
}

fn mkdir(path: &Path) -> Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<Entry> {
    let mut source = read_file(source)?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0;
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        target.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
        bytes += count as u64;
    }
    target.sync_all()?;
    Ok(Entry {
        bytes,
        sha256: digest.finalize().into(),
    })
}

fn collect(
    root: &Path,
    relative: &Path,
    dirs: &mut Vec<PathBuf>,
    files: &mut Vec<PathBuf>,
    locks: &mut Vec<File>,
) -> Result<()> {
    directory(&root.join(relative))?;
    dirs.push(relative.to_owned());
    let mut children =
        fs::read_dir(root.join(relative))?.collect::<std::result::Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let relative = relative.join(child.file_name());
        if child.file_type()?.is_dir() {
            collect(root, &relative, dirs, files, locks)?;
        } else {
            let file = read_file(&child.path())?;
            if matches!(
                child.file_name().to_str(),
                Some("spool.lock" | "writer.lock")
            ) {
                file.try_lock_exclusive()
                    .map_err(|_| invalid("spool is in use; stop the gateway before backup"))?;
                locks.push(file);
            }
            files.push(relative);
        }
    }
    Ok(())
}

pub async fn verify_database(path: &Path) -> Result<uuid::Uuid> {
    if fs::symlink_metadata(path)?.len() == 0 {
        return Err(invalid("cannot back up an uninitialized database"));
    }
    let database = SqliteDatabase::open(path).await?;
    let result = async {
        let pools = database.pools()?;
        let mut connection = pools.writer.acquire().await?;
        let count =
            super::migrations::validate_history(&mut connection, super::schema::MIGRATIONS).await?;
        if count != super::schema::MIGRATIONS.len() {
            return Err(invalid("install all supported migrations before backup"));
        }
        let checks: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&mut *connection)
            .await?;
        if checks != ["ok"] {
            return Err(invalid("SQLite integrity check failed"));
        }
        if !sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *connection)
            .await?
            .is_empty()
        {
            return Err(invalid("SQLite foreign key check failed"));
        }
        let (busy, _, _): (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut *connection)
            .await?;
        if busy != 0 {
            return Err(invalid("SQLite checkpoint is busy"));
        }
        Ok(database.database_id())
    }
    .await;
    database.close().await;
    result
}

pub async fn create(database: &Path, spool: &Path, destination: &Path) -> Result<()> {
    let destination_parent = destination
        .parent()
        .ok_or_else(|| invalid("invalid backup destination"))?;
    directory(destination_parent)?;
    if destination.exists()
        || destination.starts_with(database.parent().unwrap_or(database))
        || destination.starts_with(spool)
        || spool.starts_with(database.parent().unwrap_or(database))
        || database.parent().unwrap_or(database).starts_with(spool)
    {
        return Err(invalid(
            "backup destination must be new and disjoint from database and spool",
        ));
    }
    let database_id = verify_database(database).await?;
    let _owner = DatabaseOwner::acquire(database)?;
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut locks = Vec::new();
    collect(
        spool,
        Path::new(""),
        &mut directories,
        &mut files,
        &mut locks,
    )?;
    mkdir(destination)?;
    mkdir(&destination.join("database"))?;
    mkdir(&destination.join("spool"))?;
    let mut manifest = Manifest {
        version: 1,
        database_id,
        directories: vec![PathBuf::from("database"), PathBuf::from("spool")],
        files: BTreeMap::new(),
    };
    for suffix in ["", ".identity", "-wal", "-shm", "-journal"] {
        let source = PathBuf::from(format!("{}{suffix}", database.display()));
        if suffix.is_empty() || suffix == ".identity" || source.try_exists()? {
            let name = PathBuf::from(format!("database/gateway.sqlite{suffix}"));
            manifest
                .files
                .insert(name.clone(), copy_file(&source, &destination.join(name))?);
        }
    }
    for path in directories
        .iter()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let name = Path::new("spool").join(path);
        mkdir(&destination.join(&name))?;
        manifest.directories.push(name);
    }
    for path in files {
        let name = Path::new("spool").join(&path);
        manifest.files.insert(
            name.clone(),
            copy_file(&spool.join(path), &destination.join(name))?,
        );
    }
    for path in manifest.directories.iter().rev() {
        File::open(destination.join(path))?.sync_all()?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination.join("manifest.json"))?;
    serde_json::to_writer_pretty(&mut output, &manifest)?;
    output.sync_all()?;
    File::open(destination)?.sync_all()?;
    File::open(destination_parent)?.sync_all()?;
    Ok(())
}

fn valid_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && matches!(
            path.components()
                .next()
                .and_then(|c| c.as_os_str().to_str()),
            Some("database" | "spool")
        )
}

pub async fn restore(source: &Path, destination: &Path) -> Result<()> {
    let source_lock = directory(source)?;
    source_lock.try_lock_exclusive()?;
    let database_lock = directory(&source.join("database"))?;
    database_lock
        .try_lock_exclusive()
        .map_err(|_| invalid("snapshot database is in use"))?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("invalid restore destination"))?;
    directory(parent)?;
    if destination.exists() || destination.starts_with(source) || source.starts_with(destination) {
        return Err(invalid(
            "restore destination must be new and disjoint from snapshot",
        ));
    }
    let manifest_file = read_file(&source.join("manifest.json"))?;
    if manifest_file.metadata()?.len() > 16 * 1024 * 1024 {
        return Err(invalid("snapshot manifest too large"));
    }
    let manifest: Manifest = serde_json::from_reader(manifest_file)?;
    if manifest.version != 1
        || manifest.directories.iter().any(|p| !valid_relative(p))
        || manifest.files.keys().any(|p| !valid_relative(p))
        || !manifest
            .files
            .contains_key(Path::new("database/gateway.sqlite"))
        || !manifest
            .files
            .contains_key(Path::new("database/gateway.sqlite.identity"))
    {
        return Err(invalid("invalid snapshot manifest"));
    }
    let mut actual_dirs = Vec::new();
    let mut actual_files = Vec::new();
    let mut locks = Vec::new();
    for part in ["database", "spool"] {
        collect(
            source,
            Path::new(part),
            &mut actual_dirs,
            &mut actual_files,
            &mut locks,
        )?;
    }
    actual_dirs.sort();
    actual_files.sort();
    let mut expected_dirs = manifest.directories.clone();
    expected_dirs.sort();
    if actual_dirs != expected_dirs
        || actual_files != manifest.files.keys().cloned().collect::<Vec<_>>()
    {
        return Err(invalid("snapshot contents do not match manifest"));
    }
    // Verify before creating any destination, so corruption never produces a usable partial restore.
    for (path, expected) in &manifest.files {
        let mut file = read_file(&source.join(path))?;
        let mut digest = Sha256::new();
        let mut bytes = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
            bytes += count as u64;
        }
        let checksum: [u8; 32] = digest.finalize().into();
        if bytes != expected.bytes || checksum != expected.sha256 {
            return Err(invalid("snapshot checksum mismatch"));
        }
    }
    let staging = parent.join(format!(".sqlite-restore-{}", uuid::Uuid::new_v4()));
    mkdir(&staging)?;
    for path in &manifest.directories {
        mkdir(&staging.join(path))?;
    }
    for (path, expected) in &manifest.files {
        let copied = copy_file(&source.join(path), &staging.join(path))?;
        if copied.bytes != expected.bytes || copied.sha256 != expected.sha256 {
            return Err(invalid("snapshot changed during restore"));
        }
    }
    for path in manifest.directories.iter().rev() {
        File::open(staging.join(path))?.sync_all()?;
    }
    // Native ownership lasts until process exit. Verify in a child before publishing the
    // directory, so rename cannot move a database still claimed by this process.
    let status = tokio::process::Command::new(std::env::current_exe()?)
        .arg("verify-sqlite-restore")
        .arg(staging.join("database/gateway.sqlite"))
        .arg(manifest.database_id.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await?;
    if !status.success() {
        return Err(invalid(
            "restored database verification failed; staging retained",
        ));
    }
    for path in manifest.directories.iter().rev() {
        File::open(staging.join(path))?.sync_all()?;
    }
    File::open(&staging)?.sync_all()?;
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        &staging,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
