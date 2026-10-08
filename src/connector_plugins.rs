//! Startup-only, SHA-pinned loading of trusted native connector modules.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use ai_gateway_connector_sdk::{
    ABI_VERSION, ByteSlice, CallOutput, DispatchFn, FreeFn, MAX_BODY_BYTES, MAX_COMMAND_BYTES,
    MAX_MANIFEST_BYTES, MAX_METADATA_BYTES, OwnedBuffer, PluginCallError, PluginDescriptor,
    STATUS_ERROR, STATUS_OK,
};
pub use ai_gateway_connector_sdk::{PluginManifest, PluginOutput};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    pub id: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("connector plugin configuration is invalid: {0}")]
    Configuration(&'static str),
    #[error("connector plugin platform is unsupported")]
    UnsupportedPlatform,
    #[error("connector plugin file is inaccessible: {0}")]
    Io(#[from] std::io::Error),
    #[error("connector plugin file ownership or permissions are unsafe")]
    UnsafeFile,
    #[error("connector plugin SHA-256 does not match")]
    HashMismatch,
    #[error("connector plugin library or entry point could not be loaded")]
    Load,
    #[error("connector plugin ABI is incompatible")]
    Abi,
    #[error("connector plugin manifest is invalid")]
    Manifest,
    #[error("connector plugin command is not supported")]
    UnsupportedCommand,
    #[error("connector plugin input exceeds limits or has invalid metadata")]
    InvalidInput,
    #[error("connector plugin returned an invalid result")]
    InvalidOutput,
    #[error("connector plugin call failed")]
    CallFailed,
    #[error("connector plugin rejected the operation")]
    Rejected(String),
}

impl PluginError {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Rejected(code) => Some(code),
            _ => None,
        }
    }
}

pub struct Plugin {
    manifest: PluginManifest,
    dispatch: DispatchFn,
    free_buffer: FreeFn,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

impl Plugin {
    pub fn load(
        path: &Path,
        expected_sha256: &str,
        expected_id: &str,
    ) -> Result<Arc<Self>, PluginError> {
        validate_id(expected_id)?;
        validate_hash(expected_sha256)?;
        #[cfg(target_os = "linux")]
        {
            linux::load(path, expected_sha256, expected_id)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(PluginError::UnsupportedPlatform)
        }
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn call(
        &self,
        command: &str,
        metadata: &Value,
        body: &[u8],
    ) -> Result<PluginOutput, PluginError> {
        if !self.manifest.commands.iter().any(|value| value == command) {
            return Err(PluginError::UnsupportedCommand);
        }
        if !metadata.is_object() || body.len() > MAX_BODY_BYTES {
            return Err(PluginError::InvalidInput);
        }
        let metadata =
            Zeroizing::new(serde_json::to_vec(metadata).map_err(|_| PluginError::InvalidInput)?);
        if metadata.len() > MAX_METADATA_BYTES {
            return Err(PluginError::InvalidInput);
        }
        let mut output = CallOutput::default();
        // Only startup-verified administrator-trusted modules can supply this pointer.
        let status = unsafe {
            (self.dispatch)(
                ByteSlice::new(command.as_bytes()),
                ByteSlice::new(&metadata),
                ByteSlice::new(body),
                &mut output,
            )
        };
        validate_output_buffers(&output)?;
        let output = OutputGuard {
            output,
            free_buffer: self.free_buffer,
        };
        if status != STATUS_OK && status != STATUS_ERROR {
            return Err(PluginError::CallFailed);
        }
        let metadata = unsafe { output_bytes(output.output.metadata) };
        if status == STATUS_ERROR {
            if output.output.body.len != 0 {
                return Err(PluginError::InvalidOutput);
            }
            let mut error: PluginCallError =
                serde_json::from_slice(metadata).map_err(|_| PluginError::InvalidOutput)?;
            error.message.zeroize();
            if error.code.is_empty()
                || error.code.len() > 64
                || !error
                    .code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                error.code.zeroize();
                return Err(PluginError::InvalidOutput);
            }
            // Plugin messages can contain provider credentials or private request data.
            return Err(PluginError::Rejected(error.code));
        }
        let mut metadata: Value =
            serde_json::from_slice(metadata).map_err(|_| PluginError::InvalidOutput)?;
        if !metadata.is_object() {
            ai_gateway_connector_sdk::zeroize_json(&mut metadata);
            return Err(PluginError::InvalidOutput);
        }
        let body = unsafe { output_bytes(output.output.body) }.to_vec();
        Ok(PluginOutput { metadata, body })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConnectorPlugins {
    plugins: HashMap<String, Arc<Plugin>>,
}

impl ConnectorPlugins {
    pub fn load(configs: &[PluginConfig]) -> Result<Self, PluginError> {
        let mut ids = HashSet::new();
        for config in configs {
            validate_id(&config.id)?;
            validate_hash(&config.sha256)?;
            if !ids.insert(&config.id) {
                return Err(PluginError::Configuration("duplicate plugin ID"));
            }
        }
        let mut plugins = HashMap::new();
        for config in configs {
            plugins.insert(
                config.id.clone(),
                Plugin::load(&config.path, &config.sha256, &config.id)?,
            );
        }
        Ok(Self { plugins })
    }

    pub fn get(&self, id: &str) -> Option<Arc<Plugin>> {
        self.plugins.get(id).cloned()
    }

    pub fn supports_operation(&self, id: &str, operation: &str) -> bool {
        self.plugins.get(id).is_some_and(|plugin| {
            plugin
                .manifest
                .operations
                .iter()
                .any(|value| value == operation)
        })
    }

    pub fn manifests(&self) -> Vec<&PluginManifest> {
        let mut manifests: Vec<_> = self.plugins.values().map(|p| p.manifest()).collect();
        manifests.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        manifests
    }
}

#[cfg(test)]
pub(crate) fn test_plugins() -> ConnectorPlugins {
    use sha2::{Digest, Sha256};
    use std::sync::OnceLock;

    static PLUGINS: OnceLock<ConnectorPlugins> = OnceLock::new();
    PLUGINS
        .get_or_init(|| {
            let path = PathBuf::from(
                std::env::var_os("AI_GATEWAY_TEST_CODEX_PLUGIN")
                    .expect("run scripts/prepare-connector-tests.sh and export its plugin path"),
            );
            let sha256 = Sha256::digest(std::fs::read(&path).unwrap())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            ConnectorPlugins::load(&[PluginConfig {
                id: "codex".into(),
                path,
                sha256,
            }])
            .expect("the test Codex plugin must satisfy the production loader contract")
        })
        .clone()
}

fn validate_id(id: &str) -> Result<(), PluginError> {
    if id.is_empty()
        || id.len() > 64
        || id == "general"
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
        || !id.as_bytes()[0].is_ascii_lowercase()
    {
        return Err(PluginError::Configuration("invalid or reserved plugin ID"));
    }
    Ok(())
}

fn validate_hash(hash: &str) -> Result<(), PluginError> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PluginError::Configuration(
            "SHA-256 must contain 64 hexadecimal digits",
        ));
    }
    Ok(())
}

fn validate_manifest(manifest: &PluginManifest, expected_id: &str) -> Result<(), PluginError> {
    if manifest.id != expected_id
        || manifest.version.is_empty()
        || manifest.version.len() > 128
        || !manifest.version.is_ascii()
        || manifest.version.bytes().any(|b| b.is_ascii_control())
        || manifest.operations.is_empty()
        || manifest.operations.len() > 64
        || manifest.commands.is_empty()
        || manifest.commands.len() > 256
    {
        return Err(PluginError::Manifest);
    }
    for values in [&manifest.operations, &manifest.commands] {
        let mut seen = HashSet::new();
        for value in values {
            if value.is_empty()
                || value.len() > MAX_COMMAND_BYTES
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-./".contains(&b))
                || !seen.insert(value)
            {
                return Err(PluginError::Manifest);
            }
        }
    }
    Ok(())
}

fn validate_output_buffers(output: &CallOutput) -> Result<(), PluginError> {
    fn range(buffer: OwnedBuffer, limit: usize) -> Result<std::ops::Range<usize>, PluginError> {
        let len = usize::try_from(buffer.len).map_err(|_| PluginError::InvalidOutput)?;
        if len > limit || (len == 0) != buffer.ptr.is_null() {
            return Err(PluginError::InvalidOutput);
        }
        let start = buffer.ptr as usize;
        let end = start.checked_add(len).ok_or(PluginError::InvalidOutput)?;
        Ok(start..end)
    }
    let metadata = range(output.metadata, MAX_METADATA_BYTES)?;
    let body = range(output.body, MAX_BODY_BYTES)?;
    if !metadata.is_empty()
        && !body.is_empty()
        && metadata.start < body.end
        && body.start < metadata.end
    {
        return Err(PluginError::InvalidOutput);
    }
    Ok(())
}

unsafe fn output_bytes<'a>(buffer: OwnedBuffer) -> &'a [u8] {
    if buffer.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len as usize) }
    }
}

struct OutputGuard {
    output: CallOutput,
    free_buffer: FreeFn,
}

impl Drop for OutputGuard {
    fn drop(&mut self) {
        unsafe {
            for buffer in [self.output.metadata, self.output.body] {
                if buffer.len != 0 {
                    std::slice::from_raw_parts_mut(buffer.ptr, buffer.len as usize).zeroize();
                }
            }
            (self.free_buffer)(self.output.metadata);
            (self.free_buffer)(self.output.body);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs::{File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt, OpenOptionsExt},
        },
        path::Component,
    };

    const MAX_LIBRARY_BYTES: u64 = 256 * 1024 * 1024;

    pub(super) fn load(
        path: &Path,
        expected_sha256: &str,
        expected_id: &str,
    ) -> Result<Arc<Plugin>, PluginError> {
        let image = verified_image(path, expected_sha256)?;
        let library_path = format!("/proc/self/fd/{}", image.as_raw_fd());
        // Keep both the descriptor's code and its unique /proc fd name alive even
        // after failed initialization: plugin constructors may have spawned threads.
        let image = Box::leak(Box::new(image));
        let library =
            unsafe { libloading::Library::new(&library_path) }.map_err(|_| PluginError::Load)?;
        let library = Box::leak(Box::new(library));
        let _ = image;
        let entry = unsafe {
            library.get::<unsafe extern "C" fn() -> *const PluginDescriptor>(
                b"ai_gateway_connector_entry_v1\0",
            )
        }
        .map_err(|_| PluginError::Load)?;
        let descriptor = unsafe { entry() };
        if descriptor.is_null()
            || !(descriptor as usize).is_multiple_of(std::mem::align_of::<PluginDescriptor>())
        {
            return Err(PluginError::Abi);
        }
        // Read the fixed header before the rest; older/smaller tables are invalid.
        let version = unsafe { std::ptr::addr_of!((*descriptor).abi_version).read() };
        let size = unsafe { std::ptr::addr_of!((*descriptor).struct_size).read() };
        if version != ABI_VERSION || size != std::mem::size_of::<PluginDescriptor>() as u32 {
            return Err(PluginError::Abi);
        }
        let descriptor = unsafe { &*descriptor };
        let dispatch = descriptor.dispatch.ok_or(PluginError::Abi)?;
        let free_buffer = descriptor.free_buffer.ok_or(PluginError::Abi)?;
        let len = usize::try_from(descriptor.manifest.len).map_err(|_| PluginError::Manifest)?;
        if len == 0 || len > MAX_MANIFEST_BYTES || descriptor.manifest.ptr.is_null() {
            return Err(PluginError::Manifest);
        }
        let bytes = unsafe { std::slice::from_raw_parts(descriptor.manifest.ptr, len) };
        let manifest: PluginManifest =
            serde_json::from_slice(bytes).map_err(|_| PluginError::Manifest)?;
        validate_manifest(&manifest, expected_id)?;
        Ok(Arc::new(Plugin {
            manifest,
            dispatch,
            free_buffer,
        }))
    }

    fn verified_image(path: &Path, expected_sha256: &str) -> Result<File, PluginError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(PluginError::Configuration(
                "plugin path must be absolute and normalized",
            ));
        }
        let uid = unsafe { libc::geteuid() };
        for parent in path.ancestors().skip(1) {
            let metadata = std::fs::symlink_metadata(parent)?;
            if !metadata.is_dir()
                || metadata.mode() & 0o022 != 0
                || (metadata.uid() != 0 && metadata.uid() != uid)
            {
                return Err(PluginError::UnsafeFile);
            }
        }
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = source.metadata()?;
        if !metadata.is_file()
            || metadata.mode() & 0o222 != 0
            || (metadata.uid() != 0 && metadata.uid() != uid)
            || metadata.len() == 0
            || metadata.len() > MAX_LIBRARY_BYTES
        {
            return Err(PluginError::UnsafeFile);
        }
        let fd = unsafe {
            libc::memfd_create(
                c"ai-gateway-connector".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut image = unsafe { File::from_raw_fd(fd) };
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_LIBRARY_BYTES {
                return Err(PluginError::UnsafeFile);
            }
            hasher.update(&buffer[..count]);
            image.write_all(&buffer[..count])?;
        }
        let hash: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if !hash.eq_ignore_ascii_case(expected_sha256) {
            return Err(PluginError::HashMismatch);
        }
        let seals =
            libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
        if unsafe { libc::fcntl(image.as_raw_fd(), libc::F_ADD_SEALS, seals) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(image)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn hash_and_permissions_are_checked_before_dynamic_loading() {
            // The repository is under an owned, non-world-writable ancestor chain.
            let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let path = directory.path().join("plugin.so");
            std::fs::write(&path, b"not a library").unwrap();
            assert!(matches!(
                verified_image(&path, &"0".repeat(64)),
                Err(PluginError::UnsafeFile)
            ));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
            assert!(matches!(
                verified_image(&path, &"0".repeat(64)),
                Err(PluginError::HashMismatch)
            ));
            let hash: String = Sha256::digest(b"not a library")
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let image = verified_image(&path, &hash).unwrap();
            assert_eq!(
                unsafe { libc::fcntl(image.as_raw_fd(), libc::F_GET_SEALS) } & libc::F_SEAL_WRITE,
                libc::F_SEAL_WRITE
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_bytes_are_wiped_before_foreign_release() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static WIPED: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C" fn release(buffer: OwnedBuffer) {
            let bytes = unsafe {
                Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                    buffer.ptr,
                    buffer.len as usize,
                ))
            };
            if bytes.iter().all(|byte| *byte == 0) {
                WIPED.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn allocated() -> OwnedBuffer {
            let bytes = Box::leak(b"secret".to_vec().into_boxed_slice());
            OwnedBuffer {
                ptr: bytes.as_mut_ptr(),
                len: bytes.len() as u64,
            }
        }
        drop(OutputGuard {
            output: CallOutput {
                metadata: allocated(),
                body: allocated(),
            },
            free_buffer: release,
        });
        assert_eq!(WIPED.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn empty_registry_preserves_builtin_only_operation() {
        assert!(ConnectorPlugins::load(&[]).unwrap().get("codex").is_none());
        assert!(ConnectorPlugins::default().manifests().is_empty());
    }

    #[test]
    fn invalid_ids_hashes_and_duplicates_fail_before_loading() {
        for id in ["", "general", "../codex", "Codex", "a.b"] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_id("codex").is_ok());
        assert!(validate_hash(&"a".repeat(64)).is_ok());
        assert!(validate_hash(&"g".repeat(64)).is_err());
        let config = PluginConfig {
            id: "codex".to_owned(),
            path: PathBuf::from("/does/not/exist"),
            sha256: "0".repeat(64),
        };
        assert!(matches!(
            ConnectorPlugins::load(&[config.clone(), config]),
            Err(PluginError::Configuration("duplicate plugin ID"))
        ));
    }

    #[test]
    fn malformed_buffer_lengths_and_aliases_are_rejected() {
        let mut bytes = [1u8; 8];
        let output = CallOutput {
            metadata: OwnedBuffer {
                ptr: bytes.as_mut_ptr(),
                len: 4,
            },
            body: OwnedBuffer {
                ptr: bytes.as_mut_ptr(),
                len: 8,
            },
        };
        assert!(validate_output_buffers(&output).is_err());
        assert!(
            validate_output_buffers(&CallOutput {
                metadata: OwnedBuffer {
                    ptr: std::ptr::null_mut(),
                    len: 1
                },
                body: OwnedBuffer::default(),
            })
            .is_err()
        );
    }
}
