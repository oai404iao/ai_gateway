//! Credential-bound host context for externally implemented Codex attempts.

use std::sync::Arc;

use axum::http::{HeaderMap, Uri};
use bytes::Bytes;
use reqwest::Url;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    connector_plugins::Plugin,
    domain::{ApiOperation, CompiledChannel, RequestProtocol},
};

use super::{CodexCredentialRuntime, CodexCredentialUnavailable, CompiledCodexCredential};

#[derive(Clone)]
pub(crate) struct PreparedCodexAttempt {
    credential: Arc<CompiledCodexCredential>,
    plugin: Arc<Plugin>,
    operation: ApiOperation,
    request_context: Value,
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
            .call("attempt.capabilities", &json!({"operation":operation}), &[])
            .map_err(|_| CodexCredentialUnavailable::Unavailable)?
            .metadata;
        let capability = |name: &str| {
            capabilities[name]
                .as_bool()
                .ok_or(CodexCredentialUnavailable::Unavailable)
        };
        let request_context = plugin.call("attempt.context", &json!({
            "operation":operation,"credential_id":credential_id,"request_id":Uuid::new_v4(),
            "affinity_hash":affinity_hash,"headers":super::super::connector::plugin_header_metadata(client_headers),
        }), &[]).map_err(|_| CodexCredentialUnavailable::Unavailable)?.metadata;
        Ok(Self {
            credential: runtime.credential(credential_id, affinity_cache_hit)?,
            plugin,
            operation,
            request_context,
            preserves_affinity_on_failure: capability("preserves_affinity_on_failure")?,
            successful_response_is_sse: capability("successful_response_is_sse")?,
            changes_request_body: capability("changes_request_body")?,
        })
    }

    pub(crate) fn plugin(&self) -> &Plugin {
        &self.plugin
    }

    fn metadata(&self, protocol: RequestProtocol) -> Value {
        json!({
            "operation": self.operation,
            "protocol": protocol,
            "access_token": self.credential.access_token(),
            "account_id": self.credential.account_id(),
            "is_fedramp": self.credential.is_fedramp(),
            "request_context": self.request_context,
        })
    }

    pub(crate) fn adapt_body(
        &self,
        body: Bytes,
        protocol: RequestProtocol,
    ) -> Result<Bytes, CodexAttemptError> {
        self.plugin
            .call("attempt.body", &self.metadata(protocol), &body)
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

    pub(crate) fn inject_headers(
        &self,
        headers: &mut HeaderMap,
        protocol: RequestProtocol,
    ) -> Result<(), CodexAttemptError> {
        let mut metadata = self.metadata(protocol);
        metadata["headers"] = super::super::connector::plugin_header_metadata(headers);
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

fn plugin_error(error: crate::connector_plugins::PluginError) -> CodexAttemptError {
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
