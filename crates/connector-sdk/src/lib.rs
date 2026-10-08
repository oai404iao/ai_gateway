//! Native connector ABI and panic-contained Rust export adapter.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::panic::{AssertUnwindSafe, catch_unwind};
use zeroize::{Zeroize, Zeroizing};

thread_local! {
    static ABI_PANIC_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[doc(hidden)]
pub fn catch_plugin_panic<T>(action: impl FnOnce() -> T) -> Option<T> {
    static INSTALL_HOOK: std::sync::Once = std::sync::Once::new();
    INSTALL_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // catch_unwind alone still runs the default hook, exposing panic
            // payloads which can contain credentials or request bodies.
            if !ABI_PANIC_DEPTH
                .try_with(|depth| depth.get() != 0)
                .unwrap_or(false)
            {
                previous(info);
            }
        }));
    });
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            ABI_PANIC_DEPTH.with(|depth| depth.set(depth.get() - 1));
        }
    }
    ABI_PANIC_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let _scope = Scope;
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(value) => Some(value),
        Err(payload) => {
            // A user-defined panic payload can itself panic when dropped.
            if let Err(nested) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
                std::mem::forget(nested);
            }
            None
        }
    }
}

pub const ABI_VERSION: u32 = 1;
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_COMMAND_BYTES: usize = 128;
pub const STATUS_OK: u32 = 0;
pub const STATUS_ERROR: u32 = 1;
pub const STATUS_PANIC: u32 = 2;
pub const STATUS_INVALID: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub operations: Vec<String>,
    pub commands: Vec<String>,
}

#[derive(Clone)]
pub struct PluginOutput {
    pub metadata: Value,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for PluginOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginOutput")
            .field("metadata", &"[redacted]")
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Erase owned JSON strings and keys before releasing the value. This cannot
/// erase borrowed inputs, independent clones, or previous serializer allocations.
pub fn zeroize_json(value: &mut Value) {
    match std::mem::take(value) {
        Value::String(mut text) => text.zeroize(),
        Value::Array(mut values) => {
            for value in &mut values {
                zeroize_json(value);
            }
        }
        Value::Object(values) => {
            for (mut key, mut value) in values {
                key.zeroize();
                zeroize_json(&mut value);
            }
        }
        _ => {}
    }
}

struct OutputGuard(PluginOutput);

impl Drop for OutputGuard {
    fn drop(&mut self) {
        zeroize_json(&mut self.0.metadata);
        self.0.body.zeroize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCallError {
    pub code: String,
    pub message: String,
}

impl PluginCallError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PluginCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PluginCallError {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ByteSlice {
    pub ptr: *const u8,
    pub len: u64,
}

impl ByteSlice {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            ptr: bytes.as_ptr(),
            len: bytes.len() as u64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OwnedBuffer {
    pub ptr: *mut u8,
    pub len: u64,
}

impl Default for OwnedBuffer {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
        }
    }
}

impl OwnedBuffer {
    fn from_vec(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        let bytes = Box::leak(bytes.into_boxed_slice());
        Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len() as u64,
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct CallOutput {
    pub metadata: OwnedBuffer,
    pub body: OwnedBuffer,
}

pub type DispatchFn = unsafe extern "C" fn(ByteSlice, ByteSlice, ByteSlice, *mut CallOutput) -> u32;
pub type FreeFn = unsafe extern "C" fn(OwnedBuffer);

#[repr(C)]
pub struct PluginDescriptor {
    pub abi_version: u32,
    pub struct_size: u32,
    pub manifest: ByteSlice,
    pub dispatch: Option<DispatchFn>,
    pub free_buffer: Option<FreeFn>,
}

// Descriptors and manifest bytes are immutable and process-lived.
unsafe impl Send for PluginDescriptor {}
unsafe impl Sync for PluginDescriptor {}

/// # Safety
/// `buffer` must be an unreleased buffer returned by this library's dispatch.
pub unsafe extern "C" fn free_buffer(buffer: OwnedBuffer) {
    if buffer.len != 0 {
        unsafe {
            let mut bytes = Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                buffer.ptr,
                buffer.len as usize,
            ));
            bytes.zeroize();
        }
    }
}

unsafe fn borrowed<'a>(bytes: ByteSlice, limit: usize) -> Result<&'a [u8], ()> {
    let len = usize::try_from(bytes.len).map_err(|_| ())?;
    if len > limit || (len != 0 && bytes.ptr.is_null()) {
        return Err(());
    }
    if len == 0 {
        return Ok(&[]);
    }
    Ok(unsafe { std::slice::from_raw_parts(bytes.ptr, len) })
}

/// # Safety
/// Input pointers must be valid for their lengths until return. `output` must
/// point to writable, initialized storage owned by the host.
pub unsafe fn dispatch_adapter<F>(
    command: ByteSlice,
    metadata: ByteSlice,
    body: ByteSlice,
    output: *mut CallOutput,
    dispatch: F,
) -> u32
where
    F: FnOnce(&str, Value, &[u8]) -> Result<PluginOutput, PluginCallError>,
{
    if output.is_null() {
        return STATUS_INVALID;
    }
    unsafe { output.write(CallOutput::default()) };
    let result = catch_plugin_panic(|| {
        let command = unsafe { borrowed(command, MAX_COMMAND_BYTES) }?;
        let command = std::str::from_utf8(command).map_err(|_| ())?;
        let metadata = unsafe { borrowed(metadata, MAX_METADATA_BYTES) }?;
        let mut metadata: Value = serde_json::from_slice(metadata).map_err(|_| ())?;
        if !metadata.is_object() {
            zeroize_json(&mut metadata);
            return Err(());
        }
        let body = match unsafe { borrowed(body, MAX_BODY_BYTES) } {
            Ok(body) => body,
            Err(()) => {
                zeroize_json(&mut metadata);
                return Err(());
            }
        };
        let (status, value) = match dispatch(command, metadata, body) {
            Ok(value) => (STATUS_OK, value),
            Err(error) => (
                STATUS_ERROR,
                PluginOutput {
                    metadata: Value::Object(
                        [
                            ("code".into(), Value::String(error.code)),
                            ("message".into(), Value::String(error.message)),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                    body: Vec::new(),
                },
            ),
        };
        let mut value = OutputGuard(value);
        if !value.0.metadata.is_object() {
            return Err(());
        }
        let mut metadata = Zeroizing::new(serde_json::to_vec(&value.0.metadata).map_err(|_| ())?);
        if metadata.len() > MAX_METADATA_BYTES || value.0.body.len() > MAX_BODY_BYTES {
            return Err(());
        }
        Ok((
            status,
            CallOutput {
                metadata: OwnedBuffer::from_vec(std::mem::take(&mut metadata)),
                body: OwnedBuffer::from_vec(std::mem::take(&mut value.0.body)),
            },
        ))
    });
    match result {
        Some(Ok((status, value))) => {
            unsafe { output.write(value) };
            status
        }
        Some(Err(())) => STATUS_INVALID,
        None => STATUS_PANIC,
    }
}

#[doc(hidden)]
pub fn descriptor(manifest: PluginManifest, dispatch: DispatchFn) -> PluginDescriptor {
    let bytes = serde_json::to_vec(&manifest).expect("serializable manifest");
    assert!(bytes.len() <= MAX_MANIFEST_BYTES, "manifest too large");
    PluginDescriptor {
        abi_version: ABI_VERSION,
        struct_size: std::mem::size_of::<PluginDescriptor>() as u32,
        manifest: ByteSlice::new(Box::leak(bytes.into_boxed_slice())),
        dispatch: Some(dispatch),
        free_buffer: Some(free_buffer),
    }
}

/// Export the single v1 symbol. Dispatch may be called concurrently, must not
/// retain borrowed inputs, and must not unwind across the C boundary.
#[macro_export]
macro_rules! export_plugin {
    ($manifest:path, $dispatch:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn ai_gateway_connector_entry_v1() -> *const $crate::PluginDescriptor {
            unsafe extern "C" fn __ai_gateway_dispatch(
                command: $crate::ByteSlice,
                metadata: $crate::ByteSlice,
                body: $crate::ByteSlice,
                output: *mut $crate::CallOutput,
            ) -> u32 {
                unsafe { $crate::dispatch_adapter(command, metadata, body, output, $dispatch) }
            }
            static DESCRIPTOR: std::sync::OnceLock<$crate::PluginDescriptor> =
                std::sync::OnceLock::new();
            $crate::catch_plugin_panic(|| {
                DESCRIPTOR.get_or_init(|| $crate::descriptor($manifest(), __ai_gateway_dispatch))
                    as *const $crate::PluginDescriptor
            })
            .unwrap_or(std::ptr::null())
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            id: "fixture".into(),
            version: "1".into(),
            operations: vec!["responses".into()],
            commands: vec!["echo".into()],
        }
    }

    fn dispatch(_: &str, metadata: Value, body: &[u8]) -> Result<PluginOutput, PluginCallError> {
        Ok(PluginOutput {
            metadata,
            body: body.to_vec(),
        })
    }

    export_plugin!(manifest, dispatch);

    #[test]
    fn export_macro_accepts_dispatch_named_function() {
        let descriptor = unsafe { ai_gateway_connector_entry_v1().as_ref() }
            .expect("plugin descriptor initialization failed");
        assert_eq!(descriptor.abi_version, ABI_VERSION);
    }

    fn call<F>(metadata: &[u8], dispatch: F) -> (u32, CallOutput)
    where
        F: FnOnce(&str, Value, &[u8]) -> Result<PluginOutput, PluginCallError>,
    {
        let mut output = CallOutput::default();
        let status = unsafe {
            dispatch_adapter(
                ByteSlice::new(b"echo"),
                ByteSlice::new(metadata),
                ByteSlice::new(b"\0\xff"),
                &mut output,
                dispatch,
            )
        };
        (status, output)
    }

    #[test]
    fn preserves_binary_body_and_plugin_allocation_ownership() {
        let (status, output) = call(b"{}", |command, metadata, body| {
            assert_eq!(command, "echo");
            Ok(PluginOutput {
                metadata,
                body: body.to_vec(),
            })
        });
        assert_eq!(status, STATUS_OK);
        unsafe {
            assert_eq!(
                borrowed(
                    ByteSlice {
                        ptr: output.body.ptr,
                        len: output.body.len
                    },
                    10
                ),
                Ok(&b"\0\xff"[..])
            );
            free_buffer(output.metadata);
            free_buffer(output.body);
        }
    }

    #[test]
    fn contains_panics_and_rejects_nonobject_metadata() {
        let (status, output) = call(b"{}", |_, _, _| panic!("private panic detail"));
        assert_eq!(status, STATUS_PANIC);
        assert!(output.metadata.ptr.is_null());
        assert_eq!(call(b"[]", |_, _, _| unreachable!()).0, STATUS_INVALID);
    }

    #[test]
    fn panic_payload_is_suppressed_only_within_abi_calls() {
        const CHILD: &str = "AI_GATEWAY_SDK_PANIC_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let (status, _) = call(b"{}", |_, _, _| panic!("sdk-private-panic-sentinel"));
            assert_eq!(status, STATUS_PANIC);
            assert!(
                catch_plugin_panic(|| {
                    std::thread::spawn(|| panic!("sdk-other-thread-sentinel"))
                        .join()
                        .unwrap_err()
                })
                .is_some()
            );
            assert!(catch_unwind(|| panic!("sdk-outside-abi-sentinel")).is_err());
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::panic_payload_is_suppressed_only_within_abi_calls",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("sdk-private-panic-sentinel"));
        assert!(stderr.contains("sdk-other-thread-sentinel"));
        assert!(stderr.contains("sdk-outside-abi-sentinel"));
    }

    #[test]
    fn errors_have_no_body() {
        let (status, output) = call(b"{}", |_, _, _| Err(PluginCallError::new("bad", "request")));
        assert_eq!(status, STATUS_ERROR);
        assert_eq!(output.body.len, 0);
        unsafe { free_buffer(output.metadata) };
    }

    #[test]
    fn owned_json_is_erased_and_output_debug_is_redacted() {
        let mut metadata = serde_json::json!({"secret-key": ["token-value", {"nested":"secret"}]});
        let output = PluginOutput {
            metadata: metadata.clone(),
            body: b"secret-body".to_vec(),
        };
        let debug = format!("{output:?}");
        assert!(!debug.contains("token-value"));
        assert!(!debug.contains("secret-body"));
        zeroize_json(&mut metadata);
        assert!(metadata.is_null());
    }
}
