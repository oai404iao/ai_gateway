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
fn snapshot_compilation_excludes_unavailable_plugin_channels() {
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
            plugin_records: Default::default(),
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
    let unavailable = compile_runtime_config_with_plugins(
        records("fixture", Some(ApiOperation::ChatCompletions)),
        &plugins,
    )
    .unwrap();
    assert_eq!(unavailable.channels().count(), 0);
    let (_directory, path, sha256) = fixture_with_flags(1, &["-DFIXTURE_OMIT_HEADERS"]);
    let incomplete = ConnectorPlugins::load(&[PluginConfig {
        id: "fixture".into(),
        path,
        sha256,
    }])
    .unwrap();
    let unavailable = compile_runtime_config_with_plugins(
        records("fixture", Some(ApiOperation::Responses)),
        &incomplete,
    )
    .unwrap();
    assert_eq!(unavailable.channels().count(), 0);
    for id in ["unknown", "codex"] {
        assert!(compile_runtime_config_with_plugins(records(id, None), &plugins).is_ok());
    }

    let (_directory, path, sha256) = fixture_with_flags(1, &["-DFIXTURE_PROTOCOL3"]);
    let plugins = ConnectorPlugins::load(&[PluginConfig {
        id: "fixture".into(),
        path,
        sha256,
    }])
    .unwrap();
    let mut enabled = records("fixture", Some(ApiOperation::Responses));
    enabled.control_plane.channels[0].enabled = true;
    let snapshot = compile_runtime_config_with_plugins(enabled, &plugins).unwrap();
    let channel = snapshot.channels().next().unwrap();
    assert!(channel.permits_transport(ai_gateway::domain::CapabilityTransport::HttpJson));
    assert!(!channel.permits_transport(ai_gateway::domain::CapabilityTransport::HttpSse));
    assert!(snapshot.plugin_error("fixture").is_none());

    for (mode, permits_json) in [
        ("-DFIXTURE_DESCRIPTOR_MODE=9", true),
        ("-DFIXTURE_DESCRIPTOR_MODE=6", false),
    ] {
        let (_directory, path, sha256) = fixture_with_flags(1, &["-DFIXTURE_PROTOCOL3", mode]);
        let plugin = Plugin::load(&path, &sha256, "fixture").unwrap();
        let plugins = ConnectorPlugins::from_plugins([plugin]).unwrap();
        let mut enabled = records("fixture", Some(ApiOperation::Responses));
        let group = &mut enabled.control_plane.groups[0];
        group.api_format = ApiOperation::Responses.api_format().as_str().into();
        group.connector_kind = "fixture".into();
        group.request_compression = "default".into();
        let channel = &mut enabled.control_plane.channels[0];
        channel.api_operation = None;
        channel.enabled = true;
        channel.supports_websocket = true;
        channel.transports = vec![ai_gateway::domain::CapabilityTransport::HttpJson];
        let snapshot = compile_runtime_config_with_plugins(enabled, &plugins).unwrap();
        let channel = snapshot.channels().next().unwrap();
        assert_eq!(
            channel.permits_transport(ai_gateway::domain::CapabilityTransport::HttpJson),
            permits_json,
        );
        assert!(!channel.permits_transport(ai_gateway::domain::CapabilityTransport::HttpSse));
        assert!(!channel.permits_transport(ai_gateway::domain::CapabilityTransport::Websocket));
        assert!(!channel.supports_websocket());
        assert!(snapshot.plugin_error("fixture").is_none());
    }

    let (_directory, path, sha256) =
        fixture_with_flags(1, &["-DFIXTURE_PROTOCOL3", "-DFIXTURE_OMIT_RESPONSE_JSON"]);
    let plugins = ConnectorPlugins::load(&[PluginConfig {
        id: "fixture".into(),
        path,
        sha256,
    }])
    .unwrap();
    let mut enabled = records("fixture", Some(ApiOperation::Responses));
    enabled.control_plane.channels[0].enabled = true;
    let mut ordinary = enabled.control_plane.channels[0].clone();
    ordinary.id = Uuid::from_u128(8);
    ordinary.connector_kind = "general".into();
    enabled.control_plane.channels.push(ordinary);
    let snapshot = compile_runtime_config_with_plugins(enabled, &plugins).unwrap();
    assert_eq!(snapshot.channels().count(), 1);
    assert_eq!(
        snapshot.plugin_error("fixture"),
        Some("invalid_capabilities")
    );
    assert!(snapshot.plugins().get("fixture").is_none());
}

#[test]
fn semantic_descriptors_are_generation_bound_and_preserve_legacy_contracts() {
    use ai_gateway_connector_sdk::{ConnectorProtocol, ResponseMode};

    for flags in [
        vec![],
        vec!["-DFIXTURE_SETTINGS"],
        vec!["-DFIXTURE_PROTOCOL3"],
        vec!["-DFIXTURE_PROTOCOL3", "-DFIXTURE_SETTINGS"],
    ] {
        let (_directory, path, hash) = fixture_with_flags(1, &flags);
        let plugin = Plugin::load(&path, &hash, "fixture").unwrap();
        plugin.validate_attempt_contract().unwrap();
        let descriptor = plugin.attempt_descriptor("responses").unwrap();
        assert!(std::ptr::eq(
            descriptor,
            plugin.attempt_descriptor("responses").unwrap()
        ));
        assert!(!descriptor.capabilities.preserves_affinity_on_failure);
        assert!(!descriptor.capabilities.changes_request_body);
        assert_eq!(
            descriptor.protocols[0].protocol,
            ConnectorProtocol::NonStream
        );
        if plugin.manifest().protocol_version == 3 {
            assert_eq!(descriptor.protocols.len(), 1);
            assert_eq!(descriptor.protocols[0].response, ResponseMode::Json);
            let revision = plugin.with_revision(2);
            assert_ne!(plugin.generation_id(), revision.generation_id());
            assert!(!std::ptr::eq(
                descriptor,
                revision.attempt_descriptor("responses").unwrap()
            ));
            if let Some(settings) = plugin.settings_descriptor().unwrap() {
                let configured = plugin.configured(&settings.default_document(), 3).unwrap();
                assert!(!std::ptr::eq(
                    descriptor,
                    configured.attempt_descriptor("responses").unwrap()
                ));
            }
        } else {
            assert!(descriptor.capabilities.successful_response_is_sse);
            assert_eq!(descriptor.protocols[0].response, ResponseMode::Passthrough);
            assert_eq!(descriptor.protocols.len(), 2);
        }
    }
}

#[test]
fn semantic_descriptors_call_native_code_once_per_operation_and_generation() {
    let (_directory, path, hash) =
        fixture_with_flags(1, &["-DFIXTURE_PROTOCOL3", "-DFIXTURE_COUNT_DESCRIBE"]);
    let plugin = Plugin::load(&path, &hash, "fixture").unwrap();
    plugin.attempt_descriptor("responses").unwrap();
    plugin.attempt_descriptor("responses").unwrap();
    plugin.validate_attempt_contract().unwrap();
    assert_eq!(
        plugin.call("echo", &json!({}), &[]).unwrap().metadata["descriptor_calls"],
        1
    );
    let revision = plugin.with_revision(2);
    revision.validate_attempt_contract().unwrap();
    revision.attempt_descriptor("responses").unwrap();
    assert_eq!(
        revision.call("echo", &json!({}), &[]).unwrap().metadata["descriptor_calls"],
        2
    );
}

#[test]
fn malformed_semantic_descriptors_fail_closed_with_sanitized_cached_errors() {
    for flags in [
        vec!["-DFIXTURE_DESCRIPTOR_MODE=1"],
        vec!["-DFIXTURE_DESCRIPTOR_MODE=2"],
        vec!["-DFIXTURE_DESCRIPTOR_MODE=3"],
        vec!["-DFIXTURE_DESCRIPTOR_MODE=4"],
        vec!["-DFIXTURE_DESCRIPTOR_MODE=5"],
        vec!["-DFIXTURE_DESCRIPTOR_MODE=7"],
        vec!["-DFIXTURE_DESCRIPTOR_MODE=8"],
        vec!["-DFIXTURE_OMIT_DESCRIBE"],
        vec!["-DFIXTURE_OMIT_RESPONSE_JSON"],
        vec![
            "-DFIXTURE_DESCRIPTOR_MODE=6",
            "-DFIXTURE_OMIT_RESPONSE_EVENT",
        ],
        vec!["-DFIXTURE_RESPONSE_OPERATION=\"responses-ws\""],
        vec!["-DFIXTURE_RESPONSE_OPERATION=\"web_search\""],
        vec!["-DFIXTURE_RESPONSE_OPERATION=\"images_generation\""],
        vec!["-DFIXTURE_RESPONSE_OPERATION=\"images_edit\""],
        vec!["-DFIXTURE_RESPONSE_OPERATION=\"unknown\""],
    ] {
        let mut flags = flags;
        flags.push("-DFIXTURE_PROTOCOL3");
        flags.push("-DFIXTURE_COUNT_DESCRIBE");
        let (_directory, path, hash) = fixture_with_flags(1, &flags);
        let plugin = Plugin::load(&path, &hash, "fixture").unwrap();
        let operation = &plugin.manifest().operations[0];
        for _ in 0..2 {
            let error = plugin.attempt_descriptor(operation).unwrap_err();
            assert!(
                matches!(error, PluginError::InvalidCapabilities),
                "{flags:?}"
            );
            assert!(!format!("{error:?} {error}").contains("secret"));
        }
        assert!(plugin.validate_attempt_contract().is_err());
        assert_eq!(
            plugin.call("echo", &json!({}), &[]).unwrap().metadata["descriptor_calls"],
            if flags.contains(&"-DFIXTURE_OMIT_DESCRIBE") {
                0
            } else {
                1
            }
        );
    }
}

#[test]
fn upstream_usage_contracts_require_only_the_selected_parser_command() {
    use ai_gateway_connector_sdk::{UsageDescriptor, UsageFormat};

    let general = r#"-DFIXTURE_USAGE_DESCRIPTOR=",\"usage\":{\"parser\":\"general\",\"format\":\"anthropic_messages\"}""#;
    let custom = r#"-DFIXTURE_USAGE_DESCRIPTOR=",\"usage\":{\"parser\":\"plugin\",\"interface\":\"vendor.messages/v1\"}""#;
    for (usage, command, valid) in [
        (general, false, true),
        (custom, false, false),
        (custom, true, true),
    ] {
        let mut flags = vec![
            "-DFIXTURE_PROTOCOL3",
            "-DFIXTURE_DESCRIPTOR_MODE=10",
            "-DFIXTURE_OMIT_RESPONSE_JSON",
            "-DFIXTURE_OMIT_RESPONSE_EVENT",
            usage,
        ];
        if command {
            flags.push("-DFIXTURE_USAGE_PARSE");
        }
        let (_directory, path, hash) = fixture_with_flags(1, &flags);
        let plugin = Plugin::load(&path, &hash, "fixture").unwrap();
        let descriptor = plugin.attempt_descriptor("responses");
        assert_eq!(descriptor.is_ok(), valid);
        if usage == general {
            assert_eq!(
                descriptor.unwrap().usage,
                Some(UsageDescriptor::General {
                    format: UsageFormat::AnthropicMessages,
                })
            );
        }
    }
}

#[test]
fn codex_protocol_three_accepts_usage_profiles_but_not_response_adapters() {
    let usage = r#"-DFIXTURE_USAGE_DESCRIPTOR=",\"usage\":{\"parser\":\"general\",\"format\":\"open_ai_responses\"}""#;
    for (mode, valid) in [
        ("-DFIXTURE_DESCRIPTOR_MODE=10", true),
        ("-DFIXTURE_DESCRIPTOR_MODE=0", false),
    ] {
        let (_directory, path, hash) = fixture_with_flags(
            1,
            &[
                "-DFIXTURE_PROTOCOL3",
                "-DFIXTURE_PLUGIN_ID=\"codex\"",
                mode,
                usage,
            ],
        );
        let plugin = Plugin::load(&path, &hash, "codex").unwrap();
        assert_eq!(plugin.validate_attempt_contract().is_ok(), valid);
    }
}
