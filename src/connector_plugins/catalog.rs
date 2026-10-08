//! Immutable package installation and discovery without executing discovered code.

use super::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MEMBER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MEMBERS: usize = 10_000;

#[derive(Clone, Debug, Serialize)]
pub struct PluginArtifact {
    pub id: String,
    pub version: String,
    pub digest: String,
    pub path: PathBuf,
    pub manifest: PluginManifest,
}

#[derive(Debug)]
pub struct DirectoryPluginCatalog {
    root: PathBuf,
    _owner: File,
    install: std::sync::Mutex<()>,
    loaded: std::sync::Mutex<HashMap<(String, String), LoadedArtifact>>,
}

impl Drop for DirectoryPluginCatalog {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self._owner);
    }
}

#[derive(Debug)]
struct LoadedArtifact {
    artifact: PluginArtifact,
    plugin: Arc<Plugin>,
}

#[derive(Deserialize)]
struct BuildInfo {
    schema_version: u32,
    connector_abi: u32,
    connector_version: String,
    target: String,
    library: String,
    library_sha256: String,
}

impl DirectoryPluginCatalog {
    pub fn open(root: PathBuf) -> Result<Self, PluginError> {
        if !root.is_absolute()
            || root.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(PluginError::Configuration(
                "plugin directory must be absolute",
            ));
        }
        if !root.exists() {
            private_directory(&root)?;
        }
        safe_directory(&root)?;
        for name in ["incoming", "staging", "artifacts"] {
            let path = root.join(name);
            if !path.exists() {
                private_directory(&path)?;
            }
            safe_directory(&path)?;
        }
        use std::os::unix::fs::OpenOptionsExt;
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".owner"))?;
        fs2::FileExt::try_lock_exclusive(&owner).map_err(|_| {
            PluginError::Configuration("plugin directory is already owned by another process")
        })?;
        discard_abandoned_staging(&root.join("staging"))?;
        Ok(Self {
            root,
            _owner: owner,
            install: std::sync::Mutex::new(()),
            loaded: Default::default(),
        })
    }

    pub fn directory(&self) -> &Path {
        &self.root
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_loaded(&self, digest: &str) -> bool {
        self.loaded.lock().map_or(true, |loaded| {
            loaded
                .keys()
                .any(|(_, loaded_digest)| loaded_digest == digest)
        })
    }

    pub fn discover(&self) -> Result<Vec<PluginArtifact>, PluginError> {
        safe_directory(&self.root.join("artifacts"))?;
        let mut artifacts = Vec::new();
        let mut visited = 0usize;
        for plugin in fs::read_dir(self.root.join("artifacts"))? {
            visited += 1;
            if visited > MAX_MEMBERS {
                return Err(PluginError::Configuration("too many plugin entries"));
            }
            let plugin = plugin?;
            let id = plugin.file_name().to_string_lossy().into_owned();
            if validate_id(&id).is_err() || !plugin.file_type()?.is_dir() {
                continue;
            }
            for version in fs::read_dir(plugin.path())? {
                visited += 1;
                if visited > MAX_MEMBERS {
                    return Err(PluginError::Configuration("too many plugin entries"));
                }
                let version = version?;
                let digest = version.file_name().to_string_lossy().into_owned();
                if validate_hash(&digest).is_err() || !version.file_type()?.is_dir() {
                    continue;
                }
                match self.resolve_on_disk(&id, &digest) {
                    Ok(artifact) => artifacts.push(artifact),
                    Err(error) => {
                        tracing::warn!(plugin_id = id, error = %error, "invalid plugin artifact")
                    }
                }
                if artifacts.len() > MAX_MEMBERS {
                    return Err(PluginError::Configuration("too many plugin artifacts"));
                }
            }
        }
        artifacts.sort_by(|a, b| (&a.id, &a.digest).cmp(&(&b.id, &b.digest)));
        Ok(artifacts)
    }

    pub fn resolve(&self, id: &str, digest: &str) -> Result<PluginArtifact, PluginError> {
        validate_id(id)?;
        validate_hash(digest)?;
        if let Some(loaded) = self
            .loaded
            .lock()
            .map_err(|_| PluginError::Load)?
            .get(&(id.to_owned(), digest.to_owned()))
        {
            return Ok(loaded.artifact.clone());
        }
        self.resolve_on_disk(id, digest)
    }

    fn resolve_on_disk(&self, id: &str, digest: &str) -> Result<PluginArtifact, PluginError> {
        let directory = self.root.join("artifacts").join(id).join(digest);
        safe_directory(&directory)?;
        let artifact = inspect_package(&directory)?;
        if artifact.id != id || artifact.digest != digest {
            return Err(PluginError::HashMismatch);
        }
        Ok(artifact)
    }

    pub fn load(&self, artifact: &PluginArtifact) -> Result<Arc<Plugin>, PluginError> {
        if let Some(loaded) = self
            .loaded
            .lock()
            .map_err(|_| PluginError::Load)?
            .get(&(artifact.id.clone(), artifact.digest.clone()))
        {
            return Ok(Arc::clone(&loaded.plugin));
        }
        let verified = self.resolve(&artifact.id, &artifact.digest)?;
        let plugin = Plugin::load(&verified.path, &verified.digest, &verified.id)?;
        if serde_json::to_value(plugin.manifest()).ok()
            != serde_json::to_value(&verified.manifest).ok()
        {
            return Err(PluginError::Manifest);
        }
        self.loaded.lock().map_err(|_| PluginError::Load)?.insert(
            (verified.id.clone(), verified.digest.clone()),
            LoadedArtifact {
                artifact: verified,
                plugin: Arc::clone(&plugin),
            },
        );
        Ok(plugin)
    }

    /// Installation never executes native code or changes the active generation.
    pub fn install_archive(&self, archive: &Path) -> Result<PluginArtifact, PluginError> {
        let _guard = self.install.lock().map_err(|_| PluginError::Load)?;
        let source = open_regular(archive, MAX_ARCHIVE_BYTES)?;
        safe_directory(&self.root.join("staging"))?;
        let staging = tempfile::Builder::new()
            .prefix("install-")
            .tempdir_in(self.root.join("staging"))?;
        let decoder = flate2::read::MultiGzDecoder::new(source.take(MAX_ARCHIVE_BYTES + 1));
        let mut tar = tar::Archive::new(BoundedReader {
            source: decoder,
            remaining: MAX_ARCHIVE_BYTES,
        });
        let mut seen = HashSet::new();
        let mut root_name = None;
        let mut total = 0u64;
        for item in tar.entries()? {
            let mut entry = item?;
            if seen.len() >= MAX_MEMBERS {
                return Err(PluginError::Configuration("too many archive entries"));
            }
            let kind = entry.header().entry_type();
            if !kind.is_file() && !kind.is_dir() {
                return Err(PluginError::Configuration(
                    "archive contains a nonregular entry",
                ));
            }
            if let Some(extensions) = entry.pax_extensions()? {
                for extension in extensions {
                    let extension = extension?;
                    if extension.key_bytes().starts_with(b"GNU.sparse") {
                        return Err(PluginError::Configuration(
                            "sparse archives are not supported",
                        ));
                    }
                }
            }
            let raw = entry.path_bytes();
            let name = std::str::from_utf8(&raw)
                .map_err(|_| PluginError::Configuration("archive paths must be UTF-8"))?;
            let name = if kind.is_dir() {
                name.strip_suffix('/').unwrap_or(name)
            } else {
                name
            };
            validate_member_path(name)?;
            if !seen.insert(name.to_owned()) {
                return Err(PluginError::Configuration("duplicate archive entry"));
            }
            let first = name.split('/').next().unwrap();
            match &root_name {
                Some(root) if root != first => {
                    return Err(PluginError::Configuration(
                        "archive must contain one package directory",
                    ));
                }
                None => root_name = Some(first.to_owned()),
                _ => {}
            }
            let size = entry.size();
            total = total.checked_add(size).ok_or(PluginError::InvalidInput)?;
            if size > MAX_MEMBER_BYTES || total > MAX_ARCHIVE_BYTES || (kind.is_dir() && size != 0)
            {
                return Err(PluginError::Configuration("archive exceeds size limits"));
            }
            let destination = staging.path().join(name);
            create_private_parents(
                staging.path(),
                destination.parent().ok_or(PluginError::InvalidInput)?,
            )?;
            if kind.is_dir() {
                if !destination.exists() {
                    private_directory(&destination)?;
                }
                safe_directory(&destination)?;
            } else {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
                }
                let mut file = options.open(&destination)?;
                if std::io::copy(&mut entry, &mut file)? != size {
                    return Err(PluginError::Configuration("truncated archive entry"));
                }
                file.flush()?;
                file.sync_all()?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o444))?;
                }
            }
        }
        // Force gzip checksum validation and reject hidden entries after the tar terminator.
        let mut tail = tar.into_inner();
        let mut buffer = [0u8; 8192];
        loop {
            let count = tail.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if buffer[..count].iter().any(|byte| *byte != 0) {
                return Err(PluginError::Configuration(
                    "nonzero data after archive terminator",
                ));
            }
        }
        let package = staging
            .path()
            .join(root_name.ok_or(PluginError::InvalidInput)?);
        let artifact = inspect_package(&package)?;
        verify_checksums(&package)?;
        let parent = self.root.join("artifacts").join(&artifact.id);
        if !parent.exists() {
            private_directory(&parent)?;
        }
        safe_directory(&parent)?;
        let destination = parent.join(&artifact.digest);
        if destination.exists() {
            return self.resolve(&artifact.id, &artifact.digest);
        }
        sync_directories(&package)?;
        fs::rename(&package, &destination)?;
        File::open(&parent)?.sync_all()?;
        self.resolve(&artifact.id, &artifact.digest)
    }
}

fn discard_abandoned_staging(directory: &Path) -> Result<(), PluginError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let upload = name.strip_suffix(".tar.gz").is_some_and(|id| {
            uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
        });
        let installation = name.strip_prefix("install-").is_some_and(|suffix| {
            suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
        let kind = entry.file_type()?;
        if upload && (kind.is_file() || kind.is_symlink()) {
            fs::remove_file(entry.path())?;
        } else if installation && kind.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn inspect_package(directory: &Path) -> Result<PluginArtifact, PluginError> {
    let manifest: PluginManifest = read_json(&directory.join("manifest.json"))?;
    validate_id(&manifest.id)?;
    validate_manifest(&manifest, &manifest.id)?;
    let build: BuildInfo = read_json(&directory.join("build-info.json"))?;
    validate_hash(&build.library_sha256)?;
    validate_member_path(&build.library)?;
    if build.library.contains('/')
        || !build.library.ends_with(".so")
        || build.schema_version != 1
        || build.connector_abi != ABI_VERSION
        || build.connector_version != manifest.version
        || build.target != host_target()
    {
        return Err(PluginError::Manifest);
    }
    let path = directory.join(&build.library);
    let mut file = open_regular(&path, MAX_MEMBER_BYTES)?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header)?;
    let machine = if cfg!(target_arch = "x86_64") {
        62
    } else {
        183
    };
    if header[..6] != *b"\x7fELF\x02\x01" || u16::from_le_bytes([header[18], header[19]]) != machine
    {
        return Err(PluginError::UnsupportedPlatform);
    }
    if digest_file(&path)? != build.library_sha256 {
        return Err(PluginError::HashMismatch);
    }
    Ok(PluginArtifact {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        digest: build.library_sha256,
        path,
        manifest,
    })
}

fn host_target() -> &'static str {
    if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "gnu"
    )) {
        "aarch64-unknown-linux-gnu"
    } else {
        "unsupported"
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PluginError> {
    let mut bytes = Vec::new();
    open_regular(path, MAX_MANIFEST_BYTES as u64)?
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(PluginError::Manifest);
    }
    serde_json::from_slice(&bytes).map_err(|_| PluginError::Manifest)
}

fn digest_file(path: &Path) -> Result<String, PluginError> {
    let mut file = open_regular(path, MAX_MEMBER_BYTES)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_MEMBER_BYTES {
            return Err(PluginError::UnsafeFile);
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn verify_checksums(directory: &Path) -> Result<(), PluginError> {
    let mut checksums = String::new();
    open_regular(&directory.join("SHA256SUMS"), 2 * 1024 * 1024)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut checksums)?;
    if checksums.len() > 2 * 1024 * 1024 {
        return Err(PluginError::Manifest);
    }
    let mut checked = HashSet::new();
    for line in checksums.lines() {
        let (digest, name) = line.split_once("  ").ok_or(PluginError::Manifest)?;
        validate_member_path(name)?;
        validate_hash(digest)?;
        if name == "SHA256SUMS"
            || !checked.insert(name.to_owned())
            || digest_file(&directory.join(name))? != digest
        {
            return Err(PluginError::HashMismatch);
        }
    }
    let mut files = HashSet::new();
    collect_files(directory, directory, &mut files)?;
    files.remove("SHA256SUMS");
    if checked != files
        || !files.contains("LICENSE")
        || !files.contains("THIRD_PARTY_NOTICES.md")
        || !files.iter().any(|name| name.starts_with("LICENSES/"))
    {
        return Err(PluginError::Manifest);
    }
    Ok(())
}

fn collect_files(root: &Path, path: &Path, files: &mut HashSet<String>) -> Result<(), PluginError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if entry.file_type()?.is_file() {
            files.insert(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| PluginError::UnsafeFile)?
                    .to_str()
                    .ok_or(PluginError::UnsafeFile)?
                    .to_owned(),
            );
        } else {
            return Err(PluginError::UnsafeFile);
        }
        if files.len() > MAX_MEMBERS {
            return Err(PluginError::Configuration("too many package files"));
        }
    }
    Ok(())
}

fn validate_member_path(name: &str) -> Result<(), PluginError> {
    if name.is_empty()
        || name.len() > 4096
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(char::is_control)
        || name.split('/').count() > 32
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PluginError::Configuration("unsafe archive path"));
    }
    Ok(())
}

fn open_regular(path: &Path, maximum: u64) -> Result<File, PluginError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(PluginError::UnsafeFile);
    }
    Ok(file)
}

fn private_directory(path: &Path) -> Result<(), PluginError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn safe_directory(path: &Path) -> Result<(), PluginError> {
    for parent in path.ancestors() {
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() {
            return Err(PluginError::UnsafeFile);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o022 != 0
                || (metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() })
            {
                return Err(PluginError::UnsafeFile);
            }
        }
    }
    Ok(())
}

fn create_private_parents(root: &Path, path: &Path) -> Result<(), PluginError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| PluginError::UnsafeFile)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if !current.exists() {
            private_directory(&current)?;
        }
        safe_directory(&current)?;
    }
    Ok(())
}

fn sync_directories(path: &Path) -> Result<(), PluginError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directories(&entry.path())?;
        }
    }
    File::open(path)?.sync_all()?;
    Ok(())
}

struct BoundedReader<R> {
    source: R,
    remaining: u64,
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let maximum = buffer.len().min(self.remaining.saturating_add(1) as usize);
        let count = self.source.read(&mut buffer[..maximum])?;
        if count as u64 > self.remaining {
            return Err(std::io::Error::other("archive expansion exceeds limit"));
        }
        self.remaining -= count as u64;
        Ok(count)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn plugin_directory_has_one_lifetime_owner() {
        let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let root = directory.path().join("plugins");
        let owner = DirectoryPluginCatalog::open(root.clone()).unwrap();
        let upload = root
            .join("staging")
            .join(format!("{}.tar.gz", uuid::Uuid::new_v4()));
        let partial = root.join("staging/install-abc123");
        std::fs::write(&upload, b"incomplete upload").unwrap();
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("partial.so"), b"incomplete library").unwrap();
        let unrelated = root.join("staging/operator-note");
        std::fs::write(&unrelated, b"not managed staging").unwrap();
        assert!(DirectoryPluginCatalog::open(root.clone()).is_err());
        assert!(upload.exists());
        drop(owner);
        assert!(DirectoryPluginCatalog::open(root).is_ok());
        assert!(!upload.exists());
        assert!(!partial.exists());
        assert!(unrelated.exists());
    }
    use serde_json::json;

    fn archive(path: &Path, entries: &[(&str, &[u8], tar::EntryType)]) {
        let gzip =
            flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(gzip);
        for (name, bytes, kind) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(*kind);
            let name_bytes = name.as_bytes();
            header.as_mut_bytes()[..name_bytes.len()].copy_from_slice(name_bytes);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("../../outside").unwrap();
            }
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    fn root() -> tempfile::TempDir {
        tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap()
    }

    #[test]
    fn rejects_unsafe_archive_paths_links_duplicates_and_truncation() {
        let root = root();
        let catalog = DirectoryPluginCatalog::open(root.path().join("plugins")).unwrap();
        let path = root.path().join("upload.tar.gz");
        for name in [
            "../escape",
            "/absolute",
            "pkg/../escape",
            "pkg//file",
            "pkg\\file",
            "C:drive",
        ] {
            archive(&path, &[(name, b"x", tar::EntryType::Regular)]);
            assert!(catalog.install_archive(&path).is_err(), "{name}");
        }
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Fifo,
            tar::EntryType::Char,
            tar::EntryType::Block,
            tar::EntryType::GNUSparse,
        ] {
            archive(&path, &[("pkg/link", b"", kind)]);
            assert!(catalog.install_archive(&path).is_err(), "{kind:?}");
        }
        archive(
            &path,
            &[
                ("pkg/file", b"first", tar::EntryType::Regular),
                ("pkg/file", b"second", tar::EntryType::Regular),
            ],
        );
        assert!(catalog.install_archive(&path).is_err());
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 4]).unwrap();
        assert!(catalog.install_archive(&path).is_err());
        assert!(catalog.discover().unwrap().is_empty());
    }

    #[test]
    fn installs_checksum_verified_package_without_executing_native_code() {
        let root = root();
        let catalog = DirectoryPluginCatalog::open(root.path().join("plugins")).unwrap();
        let path = root.path().join("upload.tar.gz");
        let mut elf = vec![0u8; 20];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18] = if cfg!(target_arch = "x86_64") {
            62
        } else {
            183
        };
        let digest: String = Sha256::digest(&elf)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let manifest = serde_json::to_vec(&json!({
            "id":"fixture","version":"1.0.0","operations":["responses"],"commands":["echo"]
        }))
        .unwrap();
        let info = serde_json::to_vec(&json!({
            "schema_version":1,"connector_abi":ABI_VERSION,"connector_version":"1.0.0",
            "target":host_target(),"library":"connector.so","library_sha256":digest,
        }))
        .unwrap();
        let mut files = vec![
            ("connector.so", elf),
            ("manifest.json", manifest),
            ("build-info.json", info),
            ("LICENSE", b"license".to_vec()),
            ("THIRD_PARTY_NOTICES.md", b"notices".to_vec()),
            ("LICENSES/dependency/LICENSE", b"dependency".to_vec()),
        ];
        let sums: String = files
            .iter()
            .map(|(name, bytes)| {
                let hash: String = Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                format!("{hash}  {name}\n")
            })
            .collect();
        files.push(("SHA256SUMS", sums.into_bytes()));
        let names: Vec<_> = files
            .iter()
            .map(|(name, _)| format!("package/{name}"))
            .collect();
        let entries: Vec<_> = files
            .iter()
            .zip(&names)
            .map(|((_, data), name)| (name.as_str(), data.as_slice(), tar::EntryType::Regular))
            .collect();
        archive(&path, &entries);
        let artifact = catalog.install_archive(&path).unwrap();
        assert_eq!(artifact.digest, digest);
        assert_eq!(artifact.id, "fixture");
        assert_eq!(catalog.discover().unwrap().len(), 1);
        assert!(!catalog.is_loaded(&digest));
        assert_eq!(catalog.install_archive(&path).unwrap().path, artifact.path);
        assert!(catalog.resolve("../escape", &digest).is_err());
        assert!(catalog.resolve("fixture", "../escape").is_err());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&artifact.path).unwrap().permissions().mode() & 0o222,
            0
        );
    }

    #[test]
    fn expansion_budget_applies_to_metadata_and_trailing_bytes() {
        let mut bounded = BoundedReader {
            source: &b"12345"[..],
            remaining: 4,
        };
        let mut bytes = Vec::new();
        assert!(bounded.read_to_end(&mut bytes).is_err());
    }

    #[test]
    #[ignore = "requires the prepared AI_GATEWAY_TEST_CODEX_PLUGIN and companion archive"]
    fn prepared_codex_archive_loads_and_active_image_survives_disk_removal() {
        let path = PathBuf::from(std::env::var_os("AI_GATEWAY_TEST_CODEX_PLUGIN").unwrap());
        let root = root();
        let catalog = DirectoryPluginCatalog::open(root.path().join("plugins")).unwrap();
        let artifact = catalog
            .install_archive(&path.parent().unwrap().join("codex-test.tar.gz"))
            .unwrap();
        let plugin = catalog.load(&artifact).unwrap();
        let descriptor = plugin.settings_descriptor().unwrap().unwrap();
        let configured = plugin
            .configured(&descriptor.default_document(), 7)
            .unwrap();
        assert_eq!(configured.manifest().protocol_version, 2);
        assert!(
            configured
                .call(
                    "attempt.capabilities",
                    &json!({"operation":"responses"}),
                    &[]
                )
                .is_ok()
        );
        fs::remove_file(&artifact.path).unwrap();
        assert!(catalog.discover().unwrap().is_empty());
        assert_eq!(
            catalog
                .resolve(&artifact.id, &artifact.digest)
                .unwrap()
                .digest,
            artifact.digest
        );
        assert!(
            catalog
                .load(&artifact)
                .unwrap()
                .settings_descriptor()
                .unwrap()
                .is_some()
        );
    }
}
