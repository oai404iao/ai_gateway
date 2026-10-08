#![cfg(target_os = "linux")]

use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

use ai_gateway::connector_plugins::{ConnectorPlugins, Plugin, PluginConfig, PluginError};
use serde_json::json;
use sha2::{Digest, Sha256};

fn fixture(abi: u32) -> (tempfile::TempDir, PathBuf, String) {
    fixture_with_flags(abi, &[])
}

fn fixture_with_flags(abi: u32, flags: &[&str]) -> (tempfile::TempDir, PathBuf, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir_in(&root).unwrap();
    let path = directory.path().join("fixture.so");
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"])
        .args(flags)
        .arg(format!("-DFIXTURE_ABI={abi}"))
        .arg(root.join("crates/connector-sdk/tests/fixture.c"))
        .arg("-o")
        .arg(&path)
        .output()
        .expect("C compiler required for native connector ABI tests");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let hash = Sha256::digest(std::fs::read(&path).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (directory, path, hash)
}

#[test]
fn native_abi_roundtrip_errors_and_process_lifetime() {
    let (directory, path, hash) = fixture(1);
    let registry = ConnectorPlugins::load(&[PluginConfig {
        id: "fixture".into(),
        path,
        sha256: hash,
    }])
    .unwrap();
    assert!(registry.supports_operation("fixture", "responses"));
    assert!(!registry.supports_operation("fixture", "images"));
    let plugin = registry.get("fixture").unwrap();
    drop(registry);
    drop(directory);
    let metadata = json!({"request":"control"});
    let output = plugin.call("echo", &metadata, b"\0\xffraw").unwrap();
    assert_eq!(output.metadata, metadata);
    assert_eq!(output.body, b"\0\xffraw");
    assert!(matches!(
        plugin.call("not_declared", &metadata, b""),
        Err(PluginError::UnsupportedCommand)
    ));
    assert!(matches!(
        plugin.call("echo", &json!([]), b""),
        Err(PluginError::InvalidInput)
    ));
    let error = plugin.call("error", &metadata, b"").unwrap_err();
    assert_eq!(error.code(), Some("fixture_error"));
    assert!(!format!("{error:?} {error}").contains("secret"));
    assert!(matches!(
        plugin.call("malformed", &metadata, b""),
        Err(PluginError::InvalidOutput)
    ));
}

#[test]
fn rejects_mismatched_pin_identity_and_abi() {
    let (_directory, path, hash) = fixture(1);
    assert!(matches!(
        Plugin::load(&path, &"0".repeat(64), "fixture"),
        Err(PluginError::HashMismatch)
    ));
    assert!(matches!(
        Plugin::load(&path, &hash, "other"),
        Err(PluginError::Manifest)
    ));
    let (_directory, path, hash) = fixture(2);
    assert!(matches!(
        Plugin::load(&path, &hash, "fixture"),
        Err(PluginError::Abi)
    ));
}

#[test]
fn snapshot_compilation_requires_registered_ids_and_declared_operations() {
    use ai_gateway::{
        domain::ApiOperation,
        persistence::{
            ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, RuntimeConfigRecords,
            SystemSessionAffinitySettingsInput, SystemSettingsRecord,
        },
        runtime_config::compile_runtime_config_with_plugins,
    };
    use uuid::Uuid;

    let (_directory, path, sha256) = fixture(1);
    let plugins = ConnectorPlugins::load(&[PluginConfig {
        id: "fixture".into(),
        path,
        sha256,
    }])
    .unwrap();
    let records = |connector: &str, operation: Option<ApiOperation>| {
        let mut control_plane = ControlPlaneRecords::default();
        if let Some(operation) = operation {
            control_plane.groups.push(ChannelGroupRecord {
                id: Uuid::from_u128(1),
                name: "group".into(),
                api_format: String::new(),
                connector_kind: String::new(),
                request_compression: String::new(),
                sharing_only: false,
                enabled: true,
            });
            control_plane.channels.push(ChannelRecord {
                id: Uuid::from_u128(2),
                channel_group_id: Uuid::from_u128(1),
                api_format: operation.api_format().as_str().into(),
                logical_channel_id: Uuid::from_u128(3),
                access_id: Uuid::from_u128(4),
                api_operation: Some(operation),
                connector_kind: connector.into(),
                request_compression: "default".into(),
                access_revision: Uuid::from_u128(5),
                capability_revision: Uuid::from_u128(6),
                transports: operation.transports().to_vec(),
                name: "draft".into(),
                base_url: "https://upstream.test/base".into(),
                enabled: false,
                supports_websocket: operation == ApiOperation::ResponsesWebSocket,
                supports_standalone_web_search: operation == ApiOperation::StandaloneWebSearch,
                auto_disabled: false,
                auto_disable_allowed: false,
                billing_multiplier: 1.into(),
                proxy_id: None,
                config_template_id: None,
                override_document: json!({}),
                connect_timeout_ms: None,
                response_header_timeout_ms: None,
                stream_idle_timeout_ms: None,
                upstream_auth_kind: "none".into(),
                upstream_auth_header_name: None,
                upstream_api_key: None,
                available_models: vec![],
                test_model: None,
                test_pricing_model_id: None,
                credential: None,
                credential_binding_revision: Uuid::from_u128(7),
            });
        }
        RuntimeConfigRecords {
            control_plane,
            connector_ids: vec![connector.into()],
            sharing: vec![],
            sharing_only_channels: vec![],
            system_settings: SystemSettingsRecord {
                setting_key: ai_gateway::persistence::FORWARDING_SETTINGS_KEY.into(),
                value: json!({
                    "upstream": { "connect_timeout_seconds": 5, "response_header_timeout_seconds": 30, "stream_idle_timeout_seconds": 30 },
                    "passive_health": { "connection_failure_threshold": 3, "cooldown_seconds": 60 },
                    "session_affinity": serde_json::to_value(SystemSessionAffinitySettingsInput::default()).unwrap(),
                }),
                updated_at: chrono::Utc::now(),
            },
        }
    };
    let snapshot = compile_runtime_config_with_plugins(records("fixture", None), &plugins).unwrap();
    assert_eq!(snapshot.channels().count(), 0);
    assert!(
        compile_runtime_config_with_plugins(
            records("fixture", Some(ApiOperation::Responses)),
            &plugins
        )
        .is_ok()
    );
    assert!(
        compile_runtime_config_with_plugins(
            records("fixture", Some(ApiOperation::ChatCompletions)),
            &plugins
        )
        .is_err()
    );
    let (_directory, path, sha256) = fixture_with_flags(1, &["-DFIXTURE_OMIT_HEADERS"]);
    let incomplete = ConnectorPlugins::load(&[PluginConfig {
        id: "fixture".into(),
        path,
        sha256,
    }])
    .unwrap();
    assert!(
        compile_runtime_config_with_plugins(
            records("fixture", Some(ApiOperation::Responses)),
            &incomplete
        )
        .is_err()
    );
    for id in ["unknown", "codex"] {
        assert!(compile_runtime_config_with_plugins(records(id, None), &plugins).is_err());
    }
}
