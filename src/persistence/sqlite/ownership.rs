//! Cooperative process ownership of a private SQLite directory and its database inode.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use fs2::FileExt;
use uuid::Uuid;

use super::SqliteOpenError;

pub(super) struct DatabaseOwner {
    process: Arc<ProcessOwner>,
    closed: tokio::sync::watch::Sender<()>,
}

struct ProcessOwner {
    directory: File,
    database: File,
    marker: File,
    database_id: Uuid,
    path: PathBuf,
    fenced: AtomicBool,
    active: AtomicBool,
}

impl DatabaseOwner {
    pub(super) fn acquire(path: &Path) -> Result<Self, SqliteOpenError> {
        // SQLx can abandon an opening worker before installing native callbacks. Process-lifetime
        // leases also cover that gap; never close an extra main-file fd while SQLite may hold locks.
        static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Arc<ProcessOwner>>>> = OnceLock::new();
        let mut owners = OWNERS
            .get_or_init(Mutex::default)
            .lock()
            .expect("SQLite owners lock poisoned");
        if let Some(owner) = owners.get(path) {
            owner.verify()?;
            if owner.active.swap(true, Ordering::AcqRel) {
                return Err(SqliteOpenError::AlreadyOwned);
            }
            return Ok(Self {
                process: Arc::clone(owner),
                closed: tokio::sync::watch::channel(()).0,
            });
        }
        let owner = Arc::new(ProcessOwner::acquire(path)?);
        owners.insert(path.to_owned(), Arc::clone(&owner));
        Ok(Self {
            process: owner,
            closed: tokio::sync::watch::channel(()).0,
        })
    }

    pub(super) fn closed(&self) -> tokio::sync::watch::Receiver<()> {
        self.closed.subscribe()
    }

    pub(super) fn path(&self) -> &Path {
        &self.process.path
    }

    pub(super) fn database_id(&self) -> Uuid {
        self.process.database_id
    }

    pub(super) fn verify(&self) -> Result<(), SqliteOpenError> {
        self.process.verify()
    }
}

impl Drop for DatabaseOwner {
    fn drop(&mut self) {
        self.process.active.store(false, Ordering::Release);
    }
}

impl ProcessOwner {
    #[cfg(target_os = "linux")]
    pub(super) fn acquire(path: &Path) -> Result<Self, SqliteOpenError> {
        use rustix::fs::{Mode, OFlags, open, openat};
        use std::os::unix::fs::MetadataExt;

        if !path.is_absolute() || path.file_name().is_none() {
            return Err(SqliteOpenError::UnsafePath);
        }
        let parent = path.parent().ok_or(SqliteOpenError::UnsafePath)?;
        if parent.canonicalize()? != parent {
            return Err(SqliteOpenError::UnsafePath);
        }
        let directory = File::from(open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let metadata = directory.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o777 != 0o700
        {
            return Err(SqliteOpenError::UnsafePath);
        }
        // ext*, XFS, Btrfs, overlay and test-only tmpfs. Overlay backing storage must be local.
        let filesystem = rustix::fs::fstatfs(&directory)?.f_type as u64;
        if !matches!(
            filesystem,
            0xef53 | 0x58465342 | 0x9123683e | 0x794c7630 | 0x01021994
        ) {
            return Err(SqliteOpenError::UnsupportedFilesystem);
        }
        lock(&directory)?;
        let filename = path.file_name().ok_or(SqliteOpenError::UnsafePath)?;
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let (database, created) = match openat(
            &directory,
            filename,
            flags | OFlags::CREATE | OFlags::EXCL,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(file) => (File::from(file), true),
            Err(rustix::io::Errno::EXIST) => (
                File::from(openat(&directory, filename, flags, Mode::empty())?),
                false,
            ),
            Err(error) => return Err(error.into()),
        };
        validate_file(&database.metadata()?)?;
        lock(&database)?;
        if created {
            database.sync_all()?;
            directory.sync_all()?;
        }
        let mut marker_name = filename.to_owned();
        marker_name.push(".identity");
        let marker_flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let (mut marker, database_id) =
            match openat(&directory, &marker_name, marker_flags, Mode::empty()) {
                Ok(marker) => {
                    let mut marker = File::from(marker);
                    validate_file(&marker.metadata()?)?;
                    let identity = read_marker(&mut marker)?;
                    (marker, identity)
                }
                Err(rustix::io::Errno::NOENT) => {
                    if database.metadata()?.len() != 0 {
                        return Err(SqliteOpenError::ForeignDatabase);
                    }
                    for suffix in ["-wal", "-shm", "-journal"] {
                        let mut sidecar = path.as_os_str().to_owned();
                        sidecar.push(suffix);
                        match fs::symlink_metadata(sidecar) {
                            Ok(metadata) => {
                                validate_file(&metadata)?;
                                return Err(SqliteOpenError::ForeignDatabase);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    let marker = File::from(openat(
                        &directory,
                        &marker_name,
                        marker_flags | OFlags::CREATE | OFlags::EXCL,
                        Mode::RUSR | Mode::WUSR,
                    )?);
                    (marker, Uuid::new_v4())
                }
                Err(error) => return Err(error.into()),
            };
        if marker.metadata()?.len() == 0 {
            marker.write_all(database_id.to_string().as_bytes())?;
        }
        // An earlier bootstrap may have written a complete marker but died before fsync.
        marker.sync_all()?;
        directory.sync_all()?;
        let owner = Self {
            directory,
            database,
            marker,
            database_id,
            path: path.to_owned(),
            fenced: AtomicBool::new(false),
            active: AtomicBool::new(true),
        };
        owner.verify()?;
        Ok(owner)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn acquire(_path: &Path) -> Result<Self, SqliteOpenError> {
        Err(SqliteOpenError::UnsupportedPlatform)
    }

    fn verify(&self) -> Result<(), SqliteOpenError> {
        if self.fenced.load(Ordering::Acquire) {
            return Err(SqliteOpenError::IdentityChanged);
        }
        if let Err(error) = self.verify_paths() {
            self.fenced.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn verify_paths(&self) -> Result<(), SqliteOpenError> {
        use std::os::unix::fs::MetadataExt;

        let parent = self.path.parent().ok_or(SqliteOpenError::UnsafePath)?;
        let directory = fs::symlink_metadata(parent)?;
        let original_directory = self.directory.metadata()?;
        if !directory.is_dir()
            || directory.dev() != original_directory.dev()
            || directory.ino() != original_directory.ino()
            || directory.uid() != rustix::process::geteuid().as_raw()
            || directory.mode() & 0o777 != 0o700
            || parent.canonicalize()? != parent
        {
            return Err(SqliteOpenError::IdentityChanged);
        }
        let database = fs::symlink_metadata(&self.path)?;
        validate_file(&database)?;
        let original = self.database.metadata()?;
        if database.dev() != original.dev() || database.ino() != original.ino() {
            return Err(SqliteOpenError::IdentityChanged);
        }
        let mut marker_path = self.path.as_os_str().to_owned();
        marker_path.push(".identity");
        let marker = fs::symlink_metadata(marker_path)?;
        validate_file(&marker)?;
        let original_marker = self.marker.metadata()?;
        if marker.dev() != original_marker.dev()
            || marker.ino() != original_marker.ino()
            || marker.len() != 36
        {
            return Err(SqliteOpenError::IdentityChanged);
        }
        let mut bytes = [0_u8; 36];
        std::os::unix::fs::FileExt::read_exact_at(&self.marker, &mut bytes, 0)?;
        if bytes.as_slice() != self.database_id.to_string().as_bytes() {
            return Err(SqliteOpenError::IdentityChanged);
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = self.path.as_os_str().to_owned();
            sidecar.push(suffix);
            match fs::symlink_metadata(sidecar) {
                Ok(metadata) => validate_file(&metadata)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn verify_paths(&self) -> Result<(), SqliteOpenError> {
        Err(SqliteOpenError::UnsupportedPlatform)
    }
}

fn read_marker(file: &mut File) -> Result<Uuid, SqliteOpenError> {
    if file.metadata()?.len() != 36 {
        return Err(SqliteOpenError::ForeignDatabase);
    }
    let mut value = [0_u8; 36];
    file.read_exact(&mut value)?;
    let text = std::str::from_utf8(&value).map_err(|_| SqliteOpenError::ForeignDatabase)?;
    let identity = Uuid::parse_str(text).map_err(|_| SqliteOpenError::ForeignDatabase)?;
    if identity.is_nil() || identity.to_string() != text {
        return Err(SqliteOpenError::ForeignDatabase);
    }
    Ok(identity)
}

fn lock(file: &File) -> Result<(), SqliteOpenError> {
    file.try_lock_exclusive().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            SqliteOpenError::AlreadyOwned
        } else {
            SqliteOpenError::Io(error)
        }
    })
}

#[cfg(target_os = "linux")]
fn validate_file(metadata: &fs::Metadata) -> Result<(), SqliteOpenError> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(SqliteOpenError::UnsafePath);
    }
    Ok(())
}
