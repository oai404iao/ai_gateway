//! Credential-bound host context for externally implemented Codex attempts.

use std::sync::Arc;

use axum::http::{HeaderMap, Uri};
use bytes::Bytes;
use reqwest::Url;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    connector_plugins::Plugin,
    domain::{
        ApiOperation, CodexOutboundIdentity, CodexRequestMetadataSettings, CompiledChannel,
        RequestProtocol,
    },
    request_policy::{CodexRequestMetadata, RequestInterface},
};

use super::{CodexCredentialRuntime, CodexCredentialUnavailable, CompiledCodexCredential};

#[derive(Clone)]
pub(crate) struct PreparedCodexAttempt {
    credential: Arc<CompiledCodexCredential>,
    plugin: Arc<Plugin>,
    operation: ApiOperation,
    identity: CodexRequestIdentity,
    outbound_identity: CodexOutboundIdentity,
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
        outbound_identity: CodexOutboundIdentity,
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
        let affinity_hash = if matches!(
            operation,
            ApiOperation::ImagesGeneration | ApiOperation::ImagesEdit
        ) {
            None
        } else {
            affinity_hash
        };
        Ok(Self {
            credential: runtime.credential(credential_id, affinity_cache_hit)?,
            plugin,
            operation,
            identity: CodexRequestIdentity::new(client_headers, affinity_hash),
            outbound_identity,
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
            "user_agent": self.outbound_identity.user_agent(),
            "originator": self.outbound_identity.originator(),
            "client_version": self.outbound_identity.client_version(),
            "session_id": self.identity.session_id,
            "thread_id": self.identity.thread_id,
            "turn_id": self.identity.turn_id,
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

    pub(crate) fn platform_installation_id(&self) -> String {
        opaque_uuid(
            self.credential.credential_id().as_bytes(),
            b"ai-gateway-codex-installation",
        )
    }

    pub(crate) fn request_metadata(
        &self,
        settings: &CodexRequestMetadataSettings,
    ) -> Option<CodexRequestMetadata> {
        Some(CodexRequestMetadata::new(
            self.platform_installation_id(),
            self.identity.session_id.clone(),
            self.identity.thread_id.clone(),
            self.identity.turn_id.clone(),
            self.identity.window_id.clone(),
            settings.workspace_path().to_owned(),
            settings.git_remote_url().to_owned(),
        ))
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

    pub(crate) fn request_interface(
        &self,
        protocol: RequestProtocol,
    ) -> Result<RequestInterface, CodexAttemptError> {
        match self.operation {
            ApiOperation::Responses | ApiOperation::ResponsesWebSocket
                if protocol == RequestProtocol::WebSocket =>
            {
                Ok(RequestInterface::ResponsesWebSocket)
            }
            ApiOperation::Responses | ApiOperation::ResponsesWebSocket => {
                Ok(RequestInterface::ResponsesHttp)
            }
            ApiOperation::StandaloneWebSearch => Ok(RequestInterface::StandaloneWebSearch),
            ApiOperation::ImagesGeneration => Ok(RequestInterface::ImagesGeneration),
            ApiOperation::ImagesEdit => Ok(RequestInterface::ImagesEdit),
            ApiOperation::ChatCompletions => Err(CodexAttemptError::UnsupportedOperation),
        }
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
        _ => CodexAttemptError::InvalidTarget,
    }
}

#[derive(Clone)]
struct CodexRequestIdentity {
    session_id: String,
    thread_id: String,
    turn_id: String,
    window_id: String,
}

impl CodexRequestIdentity {
    fn new(headers: &HeaderMap, affinity_hash: Option<[u8; 32]>) -> Self {
        let session_id = valid_identity_header(headers, "session-id");
        let thread_id = valid_identity_header(headers, "thread-id");
        let (session_id, thread_id) = match (session_id, thread_id) {
            (Some(session_id), Some(thread_id)) => (session_id, thread_id),
            (Some(session_id), None) => (session_id.clone(), session_id),
            (None, Some(thread_id)) => (thread_id.clone(), thread_id),
            (None, None) => {
                let seed = affinity_hash.unwrap_or_else(random_identity_seed);
                (
                    opaque_uuid(&seed, b"codex-session"),
                    opaque_uuid(&seed, b"codex-thread"),
                )
            }
        };
        let window_id = valid_identity_header(headers, "x-codex-window-id")
            .unwrap_or_else(|| format!("{thread_id}:0"));
        Self {
            session_id,
            thread_id,
            turn_id: Uuid::new_v4().to_string(),
            window_id,
        }
    }
}

fn random_identity_seed() -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes
}

fn valid_identity_header(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 512)
        .map(str::to_owned)
}

fn opaque_uuid(seed: &[u8], domain: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(seed);
    let digest = hasher.finalize();
    let mut value = [0; 16];
    value.copy_from_slice(&digest[..16]);
    value[6] = (value[6] & 0x0f) | 0x40;
    value[8] = (value[8] & 0x3f) | 0x80;
    Uuid::from_bytes(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn identity_is_stable_for_affinity_and_retains_supplied_session() {
        let first = CodexRequestIdentity::new(&HeaderMap::new(), Some([1; 32]));
        let second = CodexRequestIdentity::new(&HeaderMap::new(), Some([1; 32]));
        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.thread_id, second.thread_id);
        assert_ne!(first.turn_id, second.turn_id);
        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_static("caller"));
        let supplied = CodexRequestIdentity::new(&headers, None);
        assert_eq!(supplied.session_id, "caller");
        assert_eq!(supplied.thread_id, "caller");
    }
}
