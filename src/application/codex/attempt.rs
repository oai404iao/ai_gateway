//! Credential-bound host context for externally implemented Codex attempts.

use std::sync::{Arc, OnceLock};

use ai_gateway_connector_host::{Plugin, PluginError};
use axum::http::{HeaderMap, HeaderValue, Uri};
use bytes::Bytes;
use reqwest::Url;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::{ApiOperation, CompiledChannel, RequestProtocol};

use super::{CodexCredentialRuntime, CodexCredentialUnavailable, CompiledCodexCredential};

#[derive(Clone)]
pub(crate) struct PreparedCodexAttempt {
    credential: Arc<CompiledCodexCredential>,
    plugin: Arc<Plugin>,
    operation: ApiOperation,
    request_context: Value,
    routing_hint: OnceLock<Option<HeaderValue>>,
    preserves_affinity_on_failure: bool,
    successful_response_is_sse: bool,
    changes_request_body: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexAttemptError {
    StreamingRequired,
    SearchStreamingUnsupported,
    ImageStreamingUnsupported,
    UnsupportedOperation,
    InvalidRequestBody,
    InvalidTarget,
    InvalidCredentials,
    PolicyRejected {
        code: &'static str,
        param: &'static str,
    },
}

impl PreparedCodexAttempt {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        runtime: &CodexCredentialRuntime,
        plugin: Arc<Plugin>,
        credential_id: Uuid,
        operation: ApiOperation,
        affinity_cache_hit: bool,
        client_headers: &HeaderMap,
        affinity_hash: Option<[u8; 32]>,
    ) -> Result<Self, CodexCredentialUnavailable> {
        let capabilities = plugin
            .attempt_descriptor(operation.as_str())
            .map_err(|_| CodexCredentialUnavailable::Unavailable)?
            .capabilities
            .clone();
        let request_context = plugin
            .call(
                "attempt.context",
                &json!({
                    "operation":operation,"credential_id":credential_id,"request_id":Uuid::new_v4(),
                    "affinity_hash":affinity_hash,"headers":identity_headers(client_headers),
                }),
                &[],
            )
            .map_err(|_| CodexCredentialUnavailable::Unavailable)?
            .metadata;
        Ok(Self {
            credential: runtime.credential(credential_id, affinity_cache_hit)?,
            plugin,
            operation,
            request_context,
            routing_hint: OnceLock::new(),
            preserves_affinity_on_failure: capabilities.preserves_affinity_on_failure,
            successful_response_is_sse: capabilities.successful_response_is_sse,
            changes_request_body: capabilities.changes_request_body,
        })
    }

    pub(crate) fn plugin(&self) -> &Arc<Plugin> {
        &self.plugin
    }

    pub(crate) fn adapt_body(
        &self,
        body: Bytes,
        protocol: RequestProtocol,
    ) -> Result<Bytes, CodexAttemptError> {
        let metadata = body_metadata(self.operation, protocol, &self.request_context);
        self.plugin
            .call("attempt.body", &metadata, &body)
            .map(|out| Bytes::from(out.body))
            .map_err(plugin_error)
    }

    pub(crate) fn upstream_url(
        &self,
        channel: &CompiledChannel,
        uri: &Uri,
    ) -> Result<Url, CodexAttemptError> {
        let output = self.plugin.call("attempt.target", &json!({
            "operation": self.operation, "base_url": channel.base_url().as_str(), "query": uri.query(),
        }), &[]).map_err(plugin_error)?;
        let url = output.metadata["url"]
            .as_str()
            .ok_or(CodexAttemptError::InvalidTarget)?;
        super::super::connector::validate_plugin_target(channel, url)
            .map_err(|_| CodexAttemptError::InvalidTarget)
    }

    pub(crate) fn bind_response_routing(&self, body: &Value) -> Result<(), CodexAttemptError> {
        if matches!(
            self.operation,
            ApiOperation::Responses | ApiOperation::ResponsesWebSocket
        ) {
            self.routing_hint
                .set(response_routing_hint(body))
                .map_err(|_| CodexAttemptError::InvalidRequestBody)?;
        }
        Ok(())
    }

    pub(crate) fn inject_headers(
        &self,
        headers: &mut HeaderMap,
        protocol: RequestProtocol,
    ) -> Result<(), CodexAttemptError> {
        let mut metadata = body_metadata(self.operation, protocol, &self.request_context);
        metadata["headers"] = super::super::connector::plugin_header_metadata(headers);
        metadata["access_token"] = json!(self.credential.access_token());
        metadata["account_id"] = json!(self.credential.account_id());
        metadata["is_fedramp"] = json!(self.credential.is_fedramp());
        let output = self
            .plugin
            .call("attempt.headers", &metadata, &[])
            .map_err(plugin_error)?;
        let mut updated = headers.clone();
        super::super::connector::apply_plugin_headers(&mut updated, &output.metadata)
            .map_err(|_| CodexAttemptError::InvalidCredentials)?;
        if !super::credential_headers_match(
            &updated,
            Some(self.credential.access_token()),
            self.credential.account_id(),
            self.credential.is_fedramp(),
        ) {
            return Err(CodexAttemptError::InvalidCredentials);
        }
        // The hint must describe the validated body, never a caller, transform, or plugin header.
        updated.remove("x-codex-routing-hint");
        if let Some(Some(hint)) = self.routing_hint.get() {
            updated.insert("x-codex-routing-hint", hint.clone());
        }
        *headers = updated;
        Ok(())
    }

    pub(crate) fn credential_id(&self) -> Uuid {
        self.credential.credential_id()
    }

    pub(crate) fn refresh_generation(&self) -> i64 {
        self.credential.refresh_generation()
    }

    pub(crate) fn preserves_affinity_on_failure(&self) -> bool {
        self.preserves_affinity_on_failure
    }

    pub(crate) fn successful_response_is_sse(&self) -> bool {
        self.successful_response_is_sse
    }

    pub(crate) fn is_image_edit(&self) -> bool {
        self.operation == ApiOperation::ImagesEdit
    }

    pub(crate) fn changes_request_body(&self) -> bool {
        self.changes_request_body
    }
}

fn identity_headers(headers: &HeaderMap) -> Value {
    Value::Object(
        headers
            .iter()
            .filter_map(|(name, value)| match name.as_str() {
                "session-id" | "thread-id" | "x-codex-window-id" => {
                    Some((name.to_string(), Value::from(value.to_str().ok()?)))
                }
                _ => None,
            })
            .collect(),
    )
}

fn body_metadata(
    operation: ApiOperation,
    protocol: RequestProtocol,
    request_context: &Value,
) -> Value {
    json!({"operation":operation,"protocol":protocol,"request_context":request_context})
}

fn response_routing_hint(body: &Value) -> Option<HeaderValue> {
    let component = |s: &&str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_graphic() && !matches!(b, b';' | b'='))
    };
    let model = body["model"].as_str().filter(component)?;
    let hint = match &body["service_tier"] {
        Value::Null => format!("model={model}"),
        tier => format!("model={model};tier={}", tier.as_str().filter(component)?),
    };
    HeaderValue::from_str(&hint).ok()
}

fn plugin_error(error: PluginError) -> CodexAttemptError {
    match error.code() {
        Some("streaming_required") => CodexAttemptError::StreamingRequired,
        Some("search_streaming_unsupported") => CodexAttemptError::SearchStreamingUnsupported,
        Some("image_streaming_unsupported") => CodexAttemptError::ImageStreamingUnsupported,
        Some("invalid_request_body") => CodexAttemptError::InvalidRequestBody,
        Some("invalid_credentials") => CodexAttemptError::InvalidCredentials,
        Some("unsupported_operation") => CodexAttemptError::UnsupportedOperation,
        Some("codex_request_body_field_unsupported") => CodexAttemptError::PolicyRejected {
            code: "codex_request_body_field_unsupported",
            param: "body",
        },
        Some("codex_request_body_field_value_unsupported") => CodexAttemptError::PolicyRejected {
            code: "codex_request_body_field_value_unsupported",
            param: "body",
        },
        Some("codex_request_header_unsupported") => CodexAttemptError::PolicyRejected {
            code: "codex_request_header_unsupported",
            param: "headers",
        },
        Some("codex_request_header_value_unsupported") => CodexAttemptError::PolicyRejected {
            code: "codex_request_header_value_unsupported",
            param: "headers",
        },
        _ => CodexAttemptError::InvalidTarget,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_exposes_only_identity_headers_and_body_gets_no_credentials() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("session-id", "session"),
            ("thread-id", "thread"),
            ("x-codex-window-id", "window"),
            ("authorization", "Bearer downstream-secret"),
            ("x-api-key", "downstream-secret"),
            ("cookie", "id=private"),
            ("x-custom", "private"),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
        }
        assert_eq!(
            identity_headers(&headers),
            json!({"session-id":"session","thread-id":"thread","x-codex-window-id":"window"})
        );
        let context = json!({"session_id":"session"});
        for (operation, protocol) in [
            (ApiOperation::Responses, RequestProtocol::Sse),
            (ApiOperation::ResponsesWebSocket, RequestProtocol::WebSocket),
        ] {
            assert_eq!(
                body_metadata(operation, protocol, &context),
                json!({"operation":operation,"protocol":protocol,"request_context":context})
            );
        }
    }

    #[test]
    fn hint_uses_only_unambiguous_wire_model_and_tier() {
        for (body, expected) in [
            (json!({"model":"wire"}), Some("model=wire")),
            (
                json!({"model":"wire","service_tier":"priority"}),
                Some("model=wire;tier=priority"),
            ),
            (json!({"model":"wire;tier=priority"}), None),
            (json!({"model":"wire","service_tier":"x\r\ny"}), None),
            (json!({"model":"wire","service_tier":123}), None),
        ] {
            assert_eq!(
                response_routing_hint(&body)
                    .as_ref()
                    .and_then(|h| h.to_str().ok()),
                expected
            );
        }
    }
}
