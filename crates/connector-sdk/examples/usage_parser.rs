//! Pure usage normalization fixture; response bytes and all transport remain host-owned.

use ai_gateway_connector_sdk::{
    ATTEMPT_DESCRIBE, AttemptCapabilities, AttemptDescriptor, ConnectorProtocol, PluginCallError,
    PluginManifest, PluginOutput, PluginSettingsDescriptor, PluginSettingsDocument,
    ProtocolCapability, ResponseMode, SETTINGS_COMPILE, SETTINGS_DESCRIBE, SETTINGS_VALIDATE,
    USAGE_PARSE, UsageDescriptor, UsageFormat, UsageParseInput, UsageParseOutput, export_plugin,
    parse_general_usage, zeroize_json,
};
use serde_json::{Value, json};

const INTERFACE: &str = "fixture.anthropic/v1";

fn manifest() -> PluginManifest {
    PluginManifest {
        protocol_version: 3,
        id: "example-usage-parser".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        operations: vec!["chat_completion".into(), "responses".into()],
        commands: [
            "attempt.capabilities",
            ATTEMPT_DESCRIBE,
            "attempt.body",
            "attempt.target",
            "attempt.headers",
            USAGE_PARSE,
            SETTINGS_DESCRIBE,
            SETTINGS_VALIDATE,
            SETTINGS_COMPILE,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }
}

fn settings_descriptor() -> PluginSettingsDescriptor {
    serde_json::from_value(json!({
        "schema_version":1,
        "title":{"en":"Usage parser fixture"},
        "fields":[{"key":"profile","label":{"en":"Usage profile"},"required":true,"type":"enum",
            "options":[
                {"value":"openai","label":{"en":"General OpenAI"}},
                {"value":"anthropic","label":{"en":"General Anthropic"}},
                {"value":"custom","label":{"en":"Custom pure parser"}},
                {"value":"invalid_custom","label":{"en":"Invalid canonical counters"}}
            ]}],
        "defaults":{"profile":"openai"}
    }))
    .expect("static settings descriptor")
}

fn invalid() -> PluginCallError {
    PluginCallError::new("invalid_metadata", "Invalid usage fixture input")
}

fn output(metadata: impl serde::Serialize) -> Result<PluginOutput, PluginCallError> {
    Ok(PluginOutput {
        metadata: serde_json::to_value(metadata).map_err(|_| invalid())?,
        body: Vec::new(),
    })
}

fn capabilities() -> AttemptCapabilities {
    AttemptCapabilities {
        preserves_affinity_on_failure: false,
        successful_response_is_sse: false,
        changes_request_body: false,
    }
}

fn dispatch(
    command: &str,
    mut metadata: Value,
    body: &[u8],
) -> Result<PluginOutput, PluginCallError> {
    let result = process(command, &metadata, body);
    zeroize_json(&mut metadata);
    result
}

fn process(command: &str, metadata: &Value, body: &[u8]) -> Result<PluginOutput, PluginCallError> {
    if command == SETTINGS_DESCRIBE {
        return output(settings_descriptor());
    }
    if matches!(command, SETTINGS_VALIDATE | SETTINGS_COMPILE) {
        let document: PluginSettingsDocument =
            serde_json::from_value(metadata.clone()).map_err(|_| invalid())?;
        let descriptor = settings_descriptor();
        let valid = document.schema_version == descriptor.schema_version
            && descriptor.validate_values(&document.values);
        if command == SETTINGS_VALIDATE {
            return output(json!({"valid":valid,"errors":if valid {vec![]} else {
                vec![json!({"field":"profile","code":"invalid_value"})]
            }}));
        }
        if !valid {
            return Err(invalid());
        }
        return output(json!({"config":document.values}));
    }
    let operation = metadata["operation"].as_str().ok_or_else(invalid)?;
    if !matches!(operation, "chat_completion" | "responses") {
        return Err(invalid());
    }
    let defaults = settings_descriptor().defaults;
    let settings = metadata.get("settings").unwrap_or(&defaults);
    let profile = settings["profile"].as_str().ok_or_else(invalid)?;
    let usage = match profile {
        "openai" => UsageDescriptor::General {
            format: if operation == "chat_completion" {
                UsageFormat::OpenAiChatCompletions
            } else {
                UsageFormat::OpenAiResponses
            },
        },
        "anthropic" => UsageDescriptor::General {
            format: UsageFormat::AnthropicMessages,
        },
        "custom" | "invalid_custom" => UsageDescriptor::Plugin {
            interface: INTERFACE.into(),
        },
        _ => return Err(invalid()),
    };
    match command {
        ATTEMPT_DESCRIBE => output(AttemptDescriptor {
            capabilities: capabilities(),
            protocols: [ConnectorProtocol::NonStream, ConnectorProtocol::Sse]
                .into_iter()
                .map(|protocol| ProtocolCapability {
                    protocol,
                    response: ResponseMode::Passthrough,
                })
                .collect(),
            usage: Some(usage),
        }),
        "attempt.capabilities" => output(capabilities()),
        "attempt.body" => Ok(PluginOutput {
            metadata: json!({}),
            body: body.to_vec(),
        }),
        "attempt.headers" => output(json!({"set":{},"remove":[]})),
        "attempt.target" => {
            let base = metadata["base_url"].as_str().ok_or_else(invalid)?;
            let path = if operation == "chat_completion" {
                "/v1/chat/completions"
            } else {
                "/v1/responses"
            };
            if metadata["path"].as_str() != Some(path) {
                return Err(invalid());
            }
            let mut url = format!("{}{path}", base.trim_end_matches('/'));
            match &metadata["query"] {
                Value::Null => {}
                Value::String(query) => {
                    url.push('?');
                    url.push_str(query);
                }
                _ => return Err(invalid()),
            }
            output(json!({"url":url}))
        }
        USAGE_PARSE => {
            if !matches!(profile, "custom" | "invalid_custom") {
                return Err(invalid());
            }
            let input = UsageParseInput {
                operation: operation.into(),
                interface: metadata["interface"].as_str().ok_or_else(invalid)?.into(),
            };
            if input.interface != INTERFACE || !input.validate_bounds(body) {
                return Err(invalid());
            }
            let mut raw: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
            let mut usage = parse_general_usage(UsageFormat::AnthropicMessages, &raw);
            zeroize_json(&mut raw);
            if profile == "invalid_custom"
                && let Some(usage) = usage.as_mut()
            {
                usage.cached_input_tokens = usage.input_tokens.saturating_add(1);
            }
            output(UsageParseOutput { usage })
        }
        _ => Err(PluginCallError::new(
            "unsupported_command",
            "Command is not implemented",
        )),
    }
}

export_plugin!(manifest, dispatch);

#[cfg(test)]
mod tests {
    use super::*;
    use ai_gateway_connector_sdk::{ByteSlice, CallOutput, CanonicalUsage, STATUS_OK};

    fn metadata(profile: &str) -> Value {
        json!({"operation":"responses","interface":INTERFACE,"settings":{"profile":profile}})
    }

    fn canonical() -> CanonicalUsage {
        CanonicalUsage {
            input_tokens: 5,
            cached_input_tokens: 3,
            cache_write_tokens: 0,
            output_tokens: 2,
            reasoning_tokens: 0,
        }
    }

    #[test]
    fn explicit_profiles_never_enable_response_adaptation() {
        assert!(settings_descriptor().validate_descriptor());
        let manifest = manifest();
        assert!(
            !manifest
                .commands
                .iter()
                .any(|command| command.starts_with("response."))
        );
        for (profile, expected) in [
            (
                "openai",
                UsageDescriptor::General {
                    format: UsageFormat::OpenAiResponses,
                },
            ),
            (
                "anthropic",
                UsageDescriptor::General {
                    format: UsageFormat::AnthropicMessages,
                },
            ),
            (
                "custom",
                UsageDescriptor::Plugin {
                    interface: INTERFACE.into(),
                },
            ),
        ] {
            let result = dispatch(ATTEMPT_DESCRIBE, metadata(profile), &[]).unwrap();
            let result: AttemptDescriptor = serde_json::from_value(result.metadata).unwrap();
            assert!(result.validate_for_operation("responses"));
            assert_eq!(result.usage, Some(expected));
            assert!(
                result
                    .protocols
                    .iter()
                    .all(|protocol| protocol.response == ResponseMode::Passthrough)
            );
            assert!(dispatch("response.json/v1", metadata(profile), b"{}").is_err());
        }
    }

    #[test]
    fn custom_parser_reuses_general_pure_parser_and_never_fabricates_unknown_usage() {
        let raw = br#"{"input_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":0,"output_tokens":2}"#;
        let result = dispatch(USAGE_PARSE, metadata("custom"), raw).unwrap();
        assert!(result.body.is_empty());
        let result: UsageParseOutput = serde_json::from_value(result.metadata).unwrap();
        assert_eq!(result.usage, Some(canonical()));
        assert!(result.validate_bounds());
        let malformed = br#"{"input_tokens":-1,"output_tokens":2}"#;
        let result = dispatch(USAGE_PARSE, metadata("custom"), malformed).unwrap();
        let result: UsageParseOutput = serde_json::from_value(result.metadata).unwrap();
        assert!(result.usage.is_none());
        let result = dispatch(USAGE_PARSE, metadata("invalid_custom"), raw).unwrap();
        let result: UsageParseOutput = serde_json::from_value(result.metadata).unwrap();
        assert!(!result.validate_bounds());
        assert!(result.usage.unwrap().cached_input_tokens > canonical().input_tokens);
        assert!(dispatch(USAGE_PARSE, metadata("custom"), b"[]").is_err());
        assert!(dispatch(USAGE_PARSE, metadata("custom"), &vec![b' '; 65537]).is_err());
        assert!(dispatch(USAGE_PARSE, metadata("openai"), raw).is_err());
        let mut wrong = metadata("custom");
        wrong["interface"] = json!("wrong.interface/v1");
        assert!(dispatch(USAGE_PARSE, wrong, raw).is_err());
    }

    #[test]
    fn exported_custom_parser_returns_only_counters_through_native_abi() {
        let descriptor = unsafe { ai_gateway_connector_entry_v1().as_ref() }.unwrap();
        for profile in ["custom", "invalid_custom"] {
            let metadata = serde_json::to_vec(&metadata(profile)).unwrap();
            let mut result = CallOutput::default();
            let status = unsafe {
                descriptor.dispatch.unwrap()(
                    ByteSlice::new(USAGE_PARSE.as_bytes()),
                    ByteSlice::new(&metadata),
                    ByteSlice::new(
                        br#"{"input_tokens":2,"cache_read_input_tokens":3,"output_tokens":2}"#,
                    ),
                    &mut result,
                )
            };
            assert_eq!(status, STATUS_OK);
            assert_eq!(result.body.len, 0);
            let bytes = unsafe {
                std::slice::from_raw_parts(result.metadata.ptr, result.metadata.len as usize)
            };
            let parsed: UsageParseOutput = serde_json::from_slice(bytes).unwrap();
            if profile == "custom" {
                assert_eq!(parsed.usage, Some(canonical()));
            } else {
                assert!(!parsed.validate_bounds());
            }
            unsafe {
                descriptor.free_buffer.unwrap()(result.metadata);
                descriptor.free_buffer.unwrap()(result.body);
            }
        }
    }
}
