//! Minimal generic Responses connector using gateway-owned transport and auth.

use ai_gateway_connector_sdk::{
    PluginCallError, PluginManifest, PluginOutput, export_plugin, zeroize_json,
};
use serde_json::{Value, json};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "example-responses".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        operations: vec!["responses".into()],
        commands: [
            "attempt.capabilities",
            "attempt.body",
            "attempt.target",
            "attempt.headers",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
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
    if metadata["operation"].as_str() != Some("responses") {
        return Err(PluginCallError::new(
            "unsupported_operation",
            "Only Responses HTTP is implemented",
        ));
    }
    if matches!(command, "attempt.body" | "attempt.headers")
        && !matches!(metadata["protocol"].as_str(), Some("non_stream" | "sse"))
    {
        return Err(PluginCallError::new(
            "unsupported_operation",
            "Only HTTP protocols are implemented",
        ));
    }
    let output = |metadata| PluginOutput {
        metadata,
        body: Vec::new(),
    };
    match command {
        "attempt.capabilities" => Ok(output(json!({
            "preserves_affinity_on_failure": false,
            "successful_response_is_sse": false,
            "changes_request_body": false,
        }))),
        "attempt.body" => Ok(PluginOutput {
            metadata: json!({}),
            body: body.to_vec(),
        }),
        "attempt.target" => {
            let base = metadata["base_url"]
                .as_str()
                .ok_or_else(|| PluginCallError::new("invalid_metadata", "Missing base_url"))?;
            if metadata["path"].as_str() != Some("/v1/responses") {
                return Err(PluginCallError::new("invalid_metadata", "Unexpected path"));
            }
            let mut url = format!("{}/v1/responses", base.trim_end_matches('/'));
            match &metadata["query"] {
                Value::Null => {}
                Value::String(query) => {
                    url.push('?');
                    url.push_str(query);
                }
                _ => return Err(PluginCallError::new("invalid_metadata", "Invalid query")),
            }
            Ok(output(json!({"url":url})))
        }
        "attempt.headers" => Ok(output(json!({"set":{},"remove":[]}))),
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
    use ai_gateway_connector_sdk::{ABI_VERSION, ByteSlice, CallOutput, STATUS_OK};

    #[test]
    fn every_declared_command_has_the_host_shape() {
        let manifest = manifest();
        assert_eq!(manifest.operations, ["responses"]);
        let metadata = json!({
            "operation":"responses", "protocol":"sse", "headers":{},
            "base_url":"https://upstream.example/base/", "path":"/v1/responses",
            "query":"trace=1",
        });
        let body = br#"{ "model": "selected", "input": "hello", "stream": true }"#;
        for command in &manifest.commands {
            let output = dispatch(command, metadata.clone(), body).unwrap();
            assert!(output.metadata.is_object());
            match command.as_str() {
                "attempt.capabilities" => {
                    for flag in [
                        "preserves_affinity_on_failure",
                        "successful_response_is_sse",
                        "changes_request_body",
                    ] {
                        assert_eq!(output.metadata[flag], false);
                    }
                }
                "attempt.body" => assert_eq!(output.body, body),
                "attempt.target" => assert_eq!(
                    output.metadata["url"],
                    "https://upstream.example/base/v1/responses?trace=1"
                ),
                "attempt.headers" => assert_eq!(output.metadata, json!({"set":{},"remove":[]})),
                _ => unreachable!(),
            }
        }
        assert!(
            dispatch(
                "attempt.body",
                json!({"operation":"responses-ws","protocol":"websocket"}),
                body,
            )
            .is_err()
        );
    }

    #[test]
    fn exported_descriptor_and_dispatch_match_the_manifest() {
        let descriptor = unsafe { ai_gateway_connector_entry_v1().as_ref() }
            .expect("plugin descriptor initialization failed");
        assert_eq!(descriptor.abi_version, ABI_VERSION);
        let bytes = unsafe {
            std::slice::from_raw_parts(descriptor.manifest.ptr, descriptor.manifest.len as usize)
        };
        let exported: PluginManifest = serde_json::from_slice(bytes).unwrap();
        assert_eq!(exported.commands, manifest().commands);
        let mut output = CallOutput::default();
        let status = unsafe {
            descriptor.dispatch.unwrap()(
                ByteSlice::new(b"attempt.headers"),
                ByteSlice::new(
                    br#"{"operation":"responses","protocol":"non_stream","headers":{}}"#,
                ),
                ByteSlice::new(&[]),
                &mut output,
            )
        };
        assert_eq!(status, STATUS_OK);
        let metadata = unsafe {
            std::slice::from_raw_parts(output.metadata.ptr, output.metadata.len as usize)
        };
        assert_eq!(
            serde_json::from_slice::<Value>(metadata).unwrap(),
            json!({"set":{},"remove":[]}),
        );
        unsafe {
            descriptor.free_buffer.unwrap()(output.metadata);
            descriptor.free_buffer.unwrap()(output.body);
        }
    }
}
