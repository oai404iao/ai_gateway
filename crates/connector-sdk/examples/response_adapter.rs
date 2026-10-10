//! HTTP response adapter fixture; transport, framing, credentials and metering stay host-owned.

use ai_gateway_connector_sdk::{
    ATTEMPT_DESCRIBE, AttemptCapabilities, AttemptDescriptor, ConnectorProtocol,
    MAX_RESPONSE_JSON_BYTES, PluginCallError, PluginManifest, PluginOutput,
    PluginSettingsDescriptor, PluginSettingsDocument, ProtocolCapability, RESPONSE_EVENT,
    RESPONSE_JSON, ResponseEvent, ResponseEventInput, ResponseEventOutput, ResponseMode,
    ResponsePhase, SETTINGS_COMPILE, SETTINGS_DESCRIBE, SETTINGS_VALIDATE, export_plugin,
    zeroize_json,
};
use serde_json::{Value, json};

fn manifest() -> PluginManifest {
    PluginManifest {
        protocol_version: 3,
        id: "example-response-adapter".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        operations: vec!["chat_completion".into(), "responses".into()],
        commands: [
            "attempt.capabilities",
            ATTEMPT_DESCRIBE,
            "attempt.body",
            "attempt.target",
            "attempt.headers",
            RESPONSE_JSON,
            RESPONSE_EVENT,
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
        "title":{"en":"Response adapter fixture"},
        "fields":[
            {"key":"label","label":{"en":"Text prefix"},"required":true,
             "type":"string","max_length":128},
            {"key":"mode","label":{"en":"Test output mode"},"required":true,"type":"enum",
             "options":[
                 {"value":"normal","label":{"en":"Normal"}},
                 {"value":"invalid","label":{"en":"Invalid output"}},
                 {"value":"metering_tamper","label":{"en":"Invalid metering"}}
             ]},
            {"key":"supported_protocols","label":{"en":"Supported protocols"},
             "required":true,"type":"enum","options":[
                 {"value":"both","label":{"en":"JSON and SSE"}},
                 {"value":"non_stream","label":{"en":"JSON only"}},
                 {"value":"sse","label":{"en":"SSE only"}}
             ]}
        ],
        "defaults":{"label":"adapted:","mode":"normal","supported_protocols":"both"}
    }))
    .expect("static settings descriptor")
}

fn output(metadata: Value) -> PluginOutput {
    PluginOutput {
        metadata,
        body: Vec::new(),
    }
}

fn invalid() -> PluginCallError {
    PluginCallError::new("invalid_metadata", "Invalid adapter input")
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

fn capabilities() -> AttemptCapabilities {
    AttemptCapabilities {
        preserves_affinity_on_failure: false,
        successful_response_is_sse: false,
        changes_request_body: false,
    }
}

fn process(command: &str, metadata: &Value, body: &[u8]) -> Result<PluginOutput, PluginCallError> {
    if command == SETTINGS_DESCRIBE {
        return Ok(output(
            serde_json::to_value(settings_descriptor()).map_err(|_| invalid())?,
        ));
    }
    if matches!(command, SETTINGS_VALIDATE | SETTINGS_COMPILE) {
        let document: PluginSettingsDocument =
            serde_json::from_value(metadata.clone()).map_err(|_| invalid())?;
        let descriptor = settings_descriptor();
        let valid = document.schema_version == descriptor.schema_version
            && descriptor.validate_values(&document.values);
        if command == SETTINGS_VALIDATE {
            let errors: Vec<Value> = if valid {
                Vec::new()
            } else {
                descriptor
                    .fields
                    .iter()
                    .map(|field| json!({"field":field.key,"code":"invalid_value"}))
                    .collect()
            };
            return Ok(output(json!({"valid":valid,"errors":errors})));
        }
        if !valid {
            return Err(invalid());
        }
        return Ok(output(json!({"config":document.values})));
    }
    let operation = metadata["operation"].as_str().ok_or_else(invalid)?;
    if !matches!(operation, "chat_completion" | "responses") {
        return Err(PluginCallError::new(
            "unsupported_operation",
            "Only Chat and Responses HTTP are implemented",
        ));
    }
    let defaults = settings_descriptor().defaults;
    let settings = metadata.get("settings").unwrap_or(&defaults);
    let supported = settings["supported_protocols"]
        .as_str()
        .ok_or_else(invalid)?;
    let label = settings["label"].as_str().ok_or_else(invalid)?;
    let mode = settings["mode"].as_str().ok_or_else(invalid)?;
    if matches!(
        command,
        "attempt.body" | "attempt.headers" | RESPONSE_JSON | RESPONSE_EVENT
    ) {
        let protocol = metadata["protocol"].as_str().ok_or_else(invalid)?;
        if !matches!(protocol, "non_stream" | "sse")
            || (supported != "both" && protocol != supported)
        {
            return Err(PluginCallError::new(
                "unsupported_protocol",
                "Protocol is not supported by this instance",
            ));
        }
    }
    match command {
        "attempt.capabilities" => Ok(output(
            serde_json::to_value(capabilities()).map_err(|_| invalid())?,
        )),
        ATTEMPT_DESCRIBE => {
            let protocols = [
                (
                    ConnectorProtocol::NonStream,
                    ResponseMode::Json,
                    "non_stream",
                ),
                (ConnectorProtocol::Sse, ResponseMode::Sse, "sse"),
            ]
            .into_iter()
            .filter(|(_, _, name)| supported == "both" || supported == *name)
            .map(|(protocol, response, _)| ProtocolCapability { protocol, response })
            .collect();
            Ok(output(
                serde_json::to_value(AttemptDescriptor {
                    capabilities: capabilities(),
                    protocols,
                    usage: None,
                })
                .map_err(|_| invalid())?,
            ))
        }
        "attempt.body" => Ok(PluginOutput {
            metadata: json!({}),
            body: body.to_vec(),
        }),
        "attempt.headers" => Ok(output(json!({"set":{},"remove":[]}))),
        "attempt.target" => {
            let base = metadata["base_url"].as_str().ok_or_else(invalid)?;
            let path = match operation {
                "chat_completion" => "/v1/chat/completions",
                _ => "/v1/responses",
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
            Ok(output(json!({"url":url})))
        }
        RESPONSE_JSON => {
            if metadata["protocol"] != "non_stream" || body.len() > MAX_RESPONSE_JSON_BYTES {
                return Err(invalid());
            }
            if mode == "invalid" {
                return Ok(PluginOutput {
                    metadata: json!({}),
                    body: b"not JSON".to_vec(),
                });
            }
            let mut value: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
            if !value.is_object() {
                zeroize_json(&mut value);
                return Err(invalid());
            }
            prefix_json(&mut value, operation, label);
            if mode == "metering_tamper" {
                tamper_usage(&mut value);
            }
            let encoded = serde_json::to_vec(&value).map_err(|_| invalid());
            zeroize_json(&mut value);
            let encoded = encoded?;
            if encoded.len() > MAX_RESPONSE_JSON_BYTES {
                return Err(invalid());
            }
            Ok(PluginOutput {
                metadata: json!({}),
                body: encoded,
            })
        }
        RESPONSE_EVENT => adapt_event(metadata, body, label, mode),
        _ => Err(PluginCallError::new(
            "unsupported_command",
            "Command is not implemented",
        )),
    }
}

fn prefix_text(value: &mut Value, label: &str) -> bool {
    if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
        *value = Value::String(format!("{label}{text}"));
        true
    } else {
        false
    }
}

fn prefix_json(value: &mut Value, operation: &str, label: &str) {
    if operation == "chat_completion" {
        if let Some(choices) = value.get_mut("choices").and_then(Value::as_array_mut) {
            for choice in choices {
                if let Some(content) = choice.pointer_mut("/message/content") {
                    prefix_text(content, label);
                }
            }
        }
    } else if let Some(items) = value.get_mut("output").and_then(Value::as_array_mut) {
        for item in items {
            if let Some(contents) = item.get_mut("content").and_then(Value::as_array_mut) {
                for content in contents {
                    if content["type"] == "output_text"
                        && let Some(text) = content.get_mut("text")
                    {
                        prefix_text(text, label);
                    }
                }
            }
        }
    }
}

fn tamper_usage(value: &mut Value) {
    let usage = if value.get("usage").is_some() {
        value.get_mut("usage")
    } else {
        value.pointer_mut("/response/usage")
    };
    if let Some(usage) = usage.and_then(Value::as_object_mut) {
        let field = if usage.contains_key("prompt_tokens") {
            "prompt_tokens"
        } else {
            "input_tokens"
        };
        usage.insert(field.into(), json!(999_999));
    }
}

fn adapt_event(
    metadata: &Value,
    body: &[u8],
    label: &str,
    mode: &str,
) -> Result<PluginOutput, PluginCallError> {
    // Settings belong to the configured invocation, not the command's typed input.
    let mut input = metadata.clone();
    input
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("settings");
    let parsed = serde_json::from_value::<ResponseEventInput>(input.clone());
    zeroize_json(&mut input);
    let mut input = parsed.map_err(|_| invalid())?;
    if !input.validate_bounds(body) {
        zeroize_json(&mut input.state);
        return Err(invalid());
    }
    if mode == "invalid" {
        return Ok(output(json!({"state":[],"events":[]})));
    }
    if input.phase == ResponsePhase::Finish {
        return Ok(output(json!({"state":input.state,"events":[]})));
    }
    let mut data = std::str::from_utf8(body).map_err(|_| invalid())?.to_owned();
    let mut text_events = input.state["text_events"].as_u64().unwrap_or(0);
    if data != "[DONE]" {
        let mut value: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
        let prefix = format!("{label}{}:", text_events.saturating_add(1));
        let mut changed = false;
        if input.operation == "chat_completion" {
            if let Some(choices) = value.get_mut("choices").and_then(Value::as_array_mut) {
                for choice in choices {
                    if choice.get("finish_reason").is_none_or(Value::is_null)
                        && let Some(text) = choice.pointer_mut("/delta/content")
                    {
                        changed |= prefix_text(text, &prefix);
                    }
                }
            }
        } else if value["type"] == "response.output_text.delta"
            && let Some(text) = value.get_mut("delta")
        {
            changed = prefix_text(text, &prefix);
        }
        if changed {
            text_events = text_events.saturating_add(1);
        }
        if mode == "metering_tamper" {
            tamper_usage(&mut value);
            changed = true;
        }
        if changed {
            data = serde_json::to_string(&value).map_err(|_| invalid())?;
        }
        zeroize_json(&mut value);
    }
    zeroize_json(&mut input.state);
    let result = ResponseEventOutput {
        state: json!({"text_events":text_events}),
        events: vec![ResponseEvent {
            event: input.event,
            data,
            id: input.id,
        }],
    };
    if !result.validate_bounds() {
        return Err(invalid());
    }
    Ok(output(serde_json::to_value(result).map_err(|_| invalid())?))
}

export_plugin!(manifest, dispatch);

#[cfg(test)]
mod tests {
    use super::*;
    use ai_gateway_connector_sdk::{ABI_VERSION, ByteSlice, CallOutput, STATUS_OK};

    #[test]
    fn exported_descriptor_and_json_dispatch_use_native_abi_one() {
        let descriptor = unsafe { ai_gateway_connector_entry_v1().as_ref() }.unwrap();
        assert_eq!(descriptor.abi_version, ABI_VERSION);
        let bytes = unsafe {
            std::slice::from_raw_parts(descriptor.manifest.ptr, descriptor.manifest.len as usize)
        };
        let exported: PluginManifest = serde_json::from_slice(bytes).unwrap();
        assert_eq!(exported.protocol_version, 3);
        assert_eq!(exported.commands, manifest().commands);
        let mut result = CallOutput::default();
        let status = unsafe {
            descriptor.dispatch.unwrap()(
                ByteSlice::new(RESPONSE_JSON.as_bytes()),
                ByteSlice::new(
                    br#"{"operation":"chat_completion","protocol":"non_stream","status":200}"#,
                ),
                ByteSlice::new(br#"{"choices":[{"message":{"content":"hello"}}]}"#),
                &mut result,
            )
        };
        assert_eq!(status, STATUS_OK);
        let body = unsafe { std::slice::from_raw_parts(result.body.ptr, result.body.len as usize) };
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "adapted:hello");
        unsafe {
            descriptor.free_buffer.unwrap()(result.metadata);
            descriptor.free_buffer.unwrap()(result.body);
        }
    }

    #[test]
    fn settings_and_capabilities_are_explicit() {
        let descriptor = settings_descriptor();
        assert!(descriptor.validate_descriptor());
        for command in [SETTINGS_VALIDATE, SETTINGS_COMPILE] {
            let result = dispatch(
                command,
                serde_json::to_value(descriptor.default_document()).unwrap(),
                &[],
            )
            .unwrap();
            assert!(result.body.is_empty());
        }
        let result = dispatch(
            ATTEMPT_DESCRIBE,
            json!({"operation":"chat_completion","settings":{
                "label":"test:","mode":"normal","supported_protocols":"sse"
            }}),
            &[],
        )
        .unwrap();
        let result: AttemptDescriptor = serde_json::from_value(result.metadata).unwrap();
        assert!(result.validate_for_operation("chat_completion"));
        assert_eq!(result.protocols.len(), 1);
        assert_eq!(result.protocols[0].protocol, ConnectorProtocol::Sse);
        assert!(
            dispatch(
                "attempt.body",
                json!({"operation":"chat_completion","protocol":"non_stream","settings":{
                    "label":"test:","mode":"normal","supported_protocols":"sse"
                }}),
                b"{}"
            )
            .is_err()
        );
    }

    #[test]
    fn json_rewrites_only_text_and_preserves_metering() {
        for (operation, body, pointer) in [
            (
                "chat_completion",
                json!({"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}],
                       "usage":{"prompt_tokens":1,"completion_tokens":2}}),
                "/choices/0/message/content",
            ),
            (
                "responses",
                json!({"status":"completed","output":[{"content":[
                    {"type":"output_text","text":"hello"}]}],
                    "usage":{"input_tokens":1,"output_tokens":2}}),
                "/output/0/content/0/text",
            ),
        ] {
            let result = dispatch(
                RESPONSE_JSON,
                json!({"operation":operation,"protocol":"non_stream","status":200}),
                &serde_json::to_vec(&body).unwrap(),
            )
            .unwrap();
            let mut result: Value = serde_json::from_slice(&result.body).unwrap();
            assert_eq!(result.pointer(pointer).unwrap(), "adapted:hello");
            *result.pointer_mut(pointer).unwrap() = json!("hello");
            assert_eq!(result, body);
        }
        let result = dispatch(
            RESPONSE_JSON,
            json!({"operation":"chat_completion","protocol":"non_stream","status":400}),
            br#"{"error":{"code":"bad_request"}}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&result.body).unwrap(),
            json!({"error":{"code":"bad_request"}})
        );
    }

    fn event_metadata(state: Value, sequence: u64) -> Value {
        json!({"operation":"chat_completion","protocol":"sse","status":200,
               "event":null,"id":"event-id","sequence":sequence,"state":state,"phase":"event"})
    }

    #[test]
    fn stream_state_is_request_local_and_terminal_finish_are_unchanged() {
        let body = br#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#;
        let first = dispatch(RESPONSE_EVENT, event_metadata(json!({}), 0), body).unwrap();
        assert_eq!(first.metadata["events"][0]["id"], "event-id");
        assert!(
            first.metadata["events"][0]["data"]
                .as_str()
                .unwrap()
                .contains("adapted:1:hello")
        );
        let second = dispatch(
            RESPONSE_EVENT,
            event_metadata(first.metadata["state"].clone(), 1),
            body,
        )
        .unwrap();
        assert!(
            second.metadata["events"][0]["data"]
                .as_str()
                .unwrap()
                .contains("adapted:2:hello")
        );
        assert_eq!(second.metadata["state"]["text_events"], 2);
        let fresh = dispatch(RESPONSE_EVENT, event_metadata(json!({}), 0), body).unwrap();
        assert_eq!(fresh.metadata, first.metadata);
        let terminal = br#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
        for body in [terminal.as_slice(), b"[DONE]"] {
            let result = dispatch(
                RESPONSE_EVENT,
                event_metadata(second.metadata["state"].clone(), 2),
                body,
            )
            .unwrap();
            assert_eq!(
                result.metadata["events"][0]["data"],
                std::str::from_utf8(body).unwrap()
            );
        }
        let mut metadata = event_metadata(second.metadata["state"].clone(), 3);
        metadata["phase"] = json!("finish");
        let result = dispatch(RESPONSE_EVENT, metadata, &[]).unwrap();
        assert_eq!(result.metadata["events"], json!([]));
    }

    #[test]
    fn negative_modes_are_fixture_only_contract_violations() {
        let mut metadata = json!({"operation":"chat_completion","protocol":"non_stream",
            "status":200,"settings":{"label":"x:","mode":"invalid","supported_protocols":"both"}});
        let body = br#"{"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
        let result = dispatch(RESPONSE_JSON, metadata.clone(), body).unwrap();
        assert!(serde_json::from_slice::<Value>(&result.body).is_err());
        metadata["settings"]["mode"] = json!("metering_tamper");
        let result = dispatch(RESPONSE_JSON, metadata, body).unwrap();
        let result: Value = serde_json::from_slice(&result.body).unwrap();
        assert_eq!(result["usage"]["prompt_tokens"], 999_999);
        let mut metadata = event_metadata(json!({}), 0);
        metadata["settings"] = json!({"label":"x:","mode":"invalid","supported_protocols":"both"});
        let result = dispatch(RESPONSE_EVENT, metadata, body).unwrap();
        assert!(
            !serde_json::from_value::<ResponseEventOutput>(result.metadata)
                .unwrap()
                .validate_bounds()
        );
    }
}
