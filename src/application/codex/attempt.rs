use axum::http::{
    HeaderMap, HeaderValue, Uri,
    header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, USER_AGENT},
};
use bytes::Bytes;
use reqwest::Url;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    domain::{
        ApiOperation, CodexOutboundIdentity, CodexRequestMetadataSettings, CompiledChannel,
        RequestProtocol,
    },
    request_policy::{CodexRequestMetadata, RequestInterface},
};

use super::{CodexCredentialRuntime, CodexCredentialUnavailable, CompiledCodexCredential};

#[derive(Clone)]
pub(crate) struct PreparedCodexAttempt {
    credential: std::sync::Arc<CompiledCodexCredential>,
    request: CodexRequestContext,
    outbound_identity: CodexOutboundIdentity,
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

#[derive(Clone)]
enum CodexRequestContext {
    Responses(CodexRequestIdentity),
    StandaloneWebSearch(CodexRequestIdentity),
    ImagesGeneration { turn_id: String },
    ImagesEdit { turn_id: String },
    Unsupported,
}

impl PreparedCodexAttempt {
    pub(crate) fn prepare(
        runtime: &CodexCredentialRuntime,
        channel_id: Uuid,
        api_operation: ApiOperation,
        affinity_cache_hit: bool,
        client_headers: &HeaderMap,
        affinity_hash: Option<[u8; 32]>,
        outbound_identity: CodexOutboundIdentity,
    ) -> Result<Self, CodexCredentialUnavailable> {
        let request = match api_operation {
            ApiOperation::Responses => CodexRequestContext::Responses(CodexRequestIdentity::new(
                client_headers,
                affinity_hash,
            )),
            ApiOperation::StandaloneWebSearch => CodexRequestContext::StandaloneWebSearch(
                CodexRequestIdentity::new(client_headers, affinity_hash),
            ),
            ApiOperation::ImagesGeneration => CodexRequestContext::ImagesGeneration {
                turn_id: Uuid::new_v4().to_string(),
            },
            ApiOperation::ImagesEdit => CodexRequestContext::ImagesEdit {
                turn_id: Uuid::new_v4().to_string(),
            },
            ApiOperation::ChatCompletions => CodexRequestContext::Unsupported,
        };
        Ok(Self {
            credential: runtime.credential(channel_id, affinity_cache_hit)?,
            request,
            outbound_identity,
        })
    }

    pub(crate) fn adapt_body(
        &self,
        body: Bytes,
        request_protocol: RequestProtocol,
    ) -> Result<Bytes, CodexAttemptError> {
        match &self.request {
            CodexRequestContext::ImagesGeneration { .. } => {
                return if request_protocol == RequestProtocol::NonStream {
                    Ok(body)
                } else {
                    Err(CodexAttemptError::ImageStreamingUnsupported)
                };
            }
            CodexRequestContext::ImagesEdit { .. } => {
                return Err(CodexAttemptError::InvalidRequestBody);
            }
            CodexRequestContext::StandaloneWebSearch(_) => {
                return if request_protocol == RequestProtocol::NonStream {
                    Ok(body)
                } else {
                    Err(CodexAttemptError::SearchStreamingUnsupported)
                };
            }
            CodexRequestContext::Unsupported => {
                return Err(CodexAttemptError::UnsupportedOperation);
            }
            CodexRequestContext::Responses(_) => {}
        }
        if !request_protocol.is_streamed() {
            return Err(CodexAttemptError::StreamingRequired);
        }
        Ok(body)
    }

    pub(crate) fn upstream_url(
        &self,
        channel: &CompiledChannel,
        uri: &Uri,
    ) -> Result<Url, CodexAttemptError> {
        let base = channel.base_url().as_str().trim_end_matches('/');
        let query = uri
            .query()
            .map_or_else(String::new, |query| format!("?{query}"));
        let path = match &self.request {
            CodexRequestContext::Responses(_) => "responses",
            CodexRequestContext::StandaloneWebSearch(_) => "alpha/search",
            CodexRequestContext::ImagesGeneration { .. } => "images/generations",
            CodexRequestContext::ImagesEdit { .. } => "images/edits",
            CodexRequestContext::Unsupported => {
                return Err(CodexAttemptError::UnsupportedOperation);
            }
        };
        Url::parse(&format!("{base}/{path}{query}")).map_err(|_| CodexAttemptError::InvalidTarget)
    }

    pub(crate) fn inject_headers(
        &self,
        headers: &mut HeaderMap,
        request_protocol: RequestProtocol,
    ) -> Result<(), CodexAttemptError> {
        let invalid = |_| CodexAttemptError::InvalidCredentials;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.credential.access_token()))
                .map_err(invalid)?,
        );
        if let Some(account_id) = self.credential.account_id() {
            headers.insert(
                "ChatGPT-Account-ID",
                HeaderValue::from_str(account_id).map_err(invalid)?,
            );
        } else {
            headers.remove("ChatGPT-Account-ID");
        }
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(self.outbound_identity.user_agent()).map_err(invalid)?,
        );
        headers.insert(
            "originator",
            HeaderValue::from_str(self.outbound_identity.originator()).map_err(invalid)?,
        );
        headers.insert(
            "version",
            HeaderValue::from_str(self.outbound_identity.client_version()).map_err(invalid)?,
        );
        if self.credential.is_fedramp() {
            headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        } else {
            headers.remove("X-OpenAI-Fedramp");
        }
        match &self.request {
            CodexRequestContext::Responses(identity) => {
                if request_protocol == RequestProtocol::WebSocket {
                    headers.remove(ACCEPT);
                    headers.remove(ACCEPT_ENCODING);
                    headers.remove(CONTENT_ENCODING);
                    headers.remove(CONTENT_TYPE);
                } else {
                    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
                    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                }
                headers.insert(
                    "session-id",
                    HeaderValue::from_str(&identity.session_id).map_err(invalid)?,
                );
                headers.insert(
                    "thread-id",
                    HeaderValue::from_str(&identity.thread_id).map_err(invalid)?,
                );
                if !headers.contains_key("x-client-request-id") {
                    headers.insert(
                        "x-client-request-id",
                        HeaderValue::from_str(&identity.thread_id).map_err(invalid)?,
                    );
                }
                headers.remove("x-codex-image-turn-id");
            }
            CodexRequestContext::ImagesGeneration { turn_id }
            | CodexRequestContext::ImagesEdit { turn_id } => {
                headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                headers.remove("session-id");
                headers.remove("thread-id");
                headers.remove("x-client-request-id");
                headers.insert(
                    "x-codex-image-turn-id",
                    HeaderValue::from_str(turn_id).map_err(invalid)?,
                );
            }
            CodexRequestContext::StandaloneWebSearch(_) => {
                headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                headers.remove("session-id");
                headers.remove("thread-id");
                headers.remove("x-codex-image-turn-id");
            }
            CodexRequestContext::Unsupported => {
                return Err(CodexAttemptError::UnsupportedOperation);
            }
        }
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
        let identity = match &self.request {
            CodexRequestContext::Responses(identity)
            | CodexRequestContext::StandaloneWebSearch(identity) => identity,
            CodexRequestContext::ImagesGeneration { .. }
            | CodexRequestContext::ImagesEdit { .. }
            | CodexRequestContext::Unsupported => return None,
        };
        Some(CodexRequestMetadata::new(
            self.platform_installation_id(),
            identity.session_id.clone(),
            identity.thread_id.clone(),
            identity.turn_id.clone(),
            identity.window_id.clone(),
            settings.workspace_path().to_owned(),
            settings.git_remote_url().to_owned(),
        ))
    }

    pub(crate) fn refresh_generation(&self) -> i64 {
        self.credential.refresh_generation()
    }

    pub(crate) fn preserves_affinity_on_failure(&self) -> bool {
        matches!(
            self.request,
            CodexRequestContext::Responses(_) | CodexRequestContext::StandaloneWebSearch(_)
        )
    }

    pub(crate) fn successful_response_is_sse(&self) -> bool {
        matches!(self.request, CodexRequestContext::Responses(_))
    }

    pub(crate) fn is_image_edit(&self) -> bool {
        matches!(self.request, CodexRequestContext::ImagesEdit { .. })
    }

    pub(crate) fn request_interface(
        &self,
        request_protocol: RequestProtocol,
    ) -> Result<RequestInterface, CodexAttemptError> {
        match self.request {
            CodexRequestContext::Responses(_) if request_protocol == RequestProtocol::WebSocket => {
                Ok(RequestInterface::ResponsesWebSocket)
            }
            CodexRequestContext::Responses(_) => Ok(RequestInterface::ResponsesHttp),
            CodexRequestContext::StandaloneWebSearch(_) => {
                Ok(RequestInterface::StandaloneWebSearch)
            }
            CodexRequestContext::ImagesGeneration { .. } => Ok(RequestInterface::ImagesGeneration),
            CodexRequestContext::ImagesEdit { .. } => Ok(RequestInterface::ImagesEdit),
            CodexRequestContext::Unsupported => Err(CodexAttemptError::UnsupportedOperation),
        }
    }

    pub(crate) fn changes_request_body(&self) -> bool {
        matches!(
            self.request,
            CodexRequestContext::Responses(_)
                | CodexRequestContext::ImagesGeneration { .. }
                | CodexRequestContext::ImagesEdit { .. }
        )
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
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    bytes
}

fn valid_identity_header(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .map(str::to_owned)
}

fn opaque_uuid(seed: &[u8], domain: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(seed);
    let digest = hasher.finalize();
    let mut value = [0_u8; 16];
    value.copy_from_slice(&digest[..16]);
    value[6] = (value[6] & 0x0f) | 0x40;
    value[8] = (value[8] & 0x3f) | 0x80;
    Uuid::from_bytes(value).to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use serde_json::Value;

    use crate::domain::CodexOutboundIdentity;
    use crate::persistence::CodexCredentialRecord;

    use super::*;

    fn default_outbound_identity() -> CodexOutboundIdentity {
        CodexOutboundIdentity::default()
    }

    fn configured_outbound_identity() -> CodexOutboundIdentity {
        CodexOutboundIdentity::new(
            Arc::from("codex_gateway"),
            Arc::from("9.8.7"),
            Arc::from("codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway"),
        )
    }

    fn runtime() -> CodexCredentialRuntime {
        let now = Utc::now();
        let runtime = CodexCredentialRuntime::new();
        runtime.replace(vec![CodexCredentialRecord {
            channel_id: Uuid::from_u128(1),
            channel_group_id: Uuid::from_u128(2),
            connector_pool_id: Uuid::from_u128(2),
            projection_channel_ids: vec![Uuid::from_u128(1), Uuid::from_u128(3)],
            label: "credential".into(),
            email: None,
            account_id: Some("account-123".into()),
            user_id: Some("user-123".into()),
            plan_type: Some("plus".into()),
            is_fedramp: false,
            id_token: "id-token".into(),
            access_token: "access-token".into(),
            refresh_token: "refresh-token".into(),
            access_token_expires_at: None,
            last_refreshed_at: now,
            refresh_generation: 7,
            reauth_required: false,
            quota_threshold_percent: 95,
            runtime_status: "active".into(),
            quota_allowed: Some(true),
            quota_limit_reached: Some(false),
            primary_used_percent: Some(10),
            primary_window_seconds: Some(10_800),
            primary_reset_at: None,
            secondary_used_percent: None,
            secondary_window_seconds: None,
            secondary_reset_at: None,
            quota_reset_credits_available: None,
            quota_checked_at: Some(now),
            last_error_code: None,
            last_error_summary: None,
            proxy_id: None,
            weight: 100,
            enabled: true,
            available_models: vec!["gpt-5-codex".into()],
            created_at: now,
            updated_at: now,
        }]);
        runtime
    }

    #[test]
    fn request_adapter_accepts_a_policy_normalized_streaming_body() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            ApiOperation::Responses,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();
        let body = attempt
            .adapt_body(
                Bytes::from_static(br#"{"model":"gpt-5-codex","stream":true,"store":false}"#),
                RequestProtocol::Sse,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["stream"], true);
        assert_eq!(value["store"], false);
        assert_eq!(value["model"], "gpt-5-codex");
    }

    #[test]
    fn request_adapter_rejects_non_streaming_http_requests() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            ApiOperation::Responses,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();
        assert_eq!(
            attempt
                .adapt_body(
                    Bytes::from_static(br#"{"model":"gpt-5-codex"}"#),
                    RequestProtocol::NonStream,
                )
                .unwrap_err(),
            CodexAttemptError::StreamingRequired
        );
    }

    #[test]
    fn request_identity_preserves_headers_and_derives_stable_opaque_ids() {
        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_static("client-session"));
        headers.insert("thread-id", HeaderValue::from_static("client-thread"));
        let client = CodexRequestIdentity::new(&headers, None);
        assert_eq!(client.session_id, "client-session");
        assert_eq!(client.thread_id, "client-thread");

        let first = CodexRequestIdentity::new(&HeaderMap::new(), Some([2; 32]));
        let second = CodexRequestIdentity::new(&HeaderMap::new(), Some([2; 32]));
        assert_eq!(first.session_id, second.session_id);
        assert_eq!(first.thread_id, second.thread_id);
        assert_ne!(first.session_id, first.thread_id);
        assert!(Uuid::parse_str(&first.session_id).is_ok());
        assert!(Uuid::parse_str(&first.thread_id).is_ok());
    }

    #[test]
    fn platform_installation_id_is_stable_for_credential_projections() {
        let runtime = runtime();
        let responses = PreparedCodexAttempt::prepare(
            &runtime,
            Uuid::from_u128(1),
            ApiOperation::Responses,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();
        let images = PreparedCodexAttempt::prepare(
            &runtime,
            Uuid::from_u128(3),
            ApiOperation::ImagesGeneration,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();

        assert_eq!(
            responses.platform_installation_id(),
            images.platform_installation_id()
        );
        assert!(Uuid::parse_str(&responses.platform_installation_id()).is_ok());
    }

    #[test]
    fn responses_headers_use_the_snapshot_pinned_outbound_identity() {
        let identity = configured_outbound_identity();
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            ApiOperation::Responses,
            false,
            &HeaderMap::new(),
            None,
            identity.clone(),
        )
        .unwrap();
        let mut headers = HeaderMap::new();

        attempt
            .inject_headers(&mut headers, RequestProtocol::Sse)
            .unwrap();

        assert!(!headers.contains_key(ACCEPT_ENCODING));
        assert!(!headers.contains_key(CONTENT_ENCODING));
        assert_eq!(
            headers.get("version").and_then(|value| value.to_str().ok()),
            Some(identity.client_version())
        );
        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some(identity.originator())
        );
        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(identity.user_agent())
        );
        for header in [
            "x-codex-beta-features",
            "x-codex-routing-hint",
            "x-codex-turn-state",
            "x-openai-internal-codex-responses-lite",
        ] {
            assert!(
                !headers.contains_key(header),
                "{header} must not be generated"
            );
        }
    }

    #[test]
    fn personal_credentials_omit_workspace_header() {
        let now = Utc::now();
        let runtime = CodexCredentialRuntime::new();
        let mut record = runtime_record(now);
        record.account_id = None;
        runtime.replace(vec![record]);
        let attempt = PreparedCodexAttempt::prepare(
            &runtime,
            Uuid::from_u128(1),
            ApiOperation::Responses,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "ChatGPT-Account-ID",
            HeaderValue::from_static("client-controlled"),
        );

        attempt
            .inject_headers(&mut headers, RequestProtocol::Sse)
            .unwrap();

        assert!(!headers.contains_key("chatgpt-account-id"));
    }

    #[test]
    fn standalone_web_search_uses_alpha_target_and_pins_connector_identity() {
        let identity = configured_outbound_identity();
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            ApiOperation::StandaloneWebSearch,
            true,
            &HeaderMap::new(),
            Some([7; 32]),
            identity.clone(),
        )
        .unwrap();
        let original = Bytes::from_static(
            br#"{ "id" : "session-123", "model" : "gpt-5-codex", "commands" : {} }"#,
        );
        assert_eq!(
            attempt
                .adapt_body(original.clone(), RequestProtocol::NonStream)
                .unwrap(),
            original
        );
        assert_eq!(
            attempt
                .adapt_body(
                    Bytes::from_static(br#"{"model":"gpt-5-codex"}"#),
                    RequestProtocol::Sse,
                )
                .unwrap_err(),
            CodexAttemptError::SearchStreamingUnsupported
        );

        let channel = CompiledChannel::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            crate::domain::ApiFormat::OpenAiResponses,
            Url::parse("https://chatgpt.example/backend-api/codex").unwrap(),
            100,
            crate::domain::UpstreamAuth::None,
            std::collections::HashSet::new(),
        );
        let target = attempt
            .upstream_url(
                &channel,
                &"/v1/alpha/search?trace=1".parse::<Uri>().unwrap(),
            )
            .unwrap();
        assert_eq!(
            target.as_str(),
            "https://chatgpt.example/backend-api/codex/alpha/search?trace=1"
        );

        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("codex_vscode"));
        headers.insert(USER_AGENT, HeaderValue::from_static("private-client/1.0"));
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"search_context_size":"medium"}"#),
        );
        headers.insert("session-id", HeaderValue::from_static("remove-session"));
        headers.insert("thread-id", HeaderValue::from_static("remove-thread"));
        headers.insert(
            "x-client-request-id",
            HeaderValue::from_static("request-123"),
        );
        attempt
            .inject_headers(&mut headers, RequestProtocol::NonStream)
            .unwrap();

        assert_eq!(headers.get("originator").unwrap(), identity.originator());
        assert_eq!(headers.get(USER_AGENT).unwrap(), identity.user_agent());
        assert_eq!(headers.get("version").unwrap(), identity.client_version());
        assert_eq!(
            headers.get("x-codex-turn-metadata").unwrap(),
            r#"{"search_context_size":"medium"}"#
        );
        assert_eq!(headers.get("x-client-request-id").unwrap(), "request-123");
        assert_eq!(headers.get(ACCEPT).unwrap(), "application/json");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
        assert!(!headers.contains_key(CONTENT_ENCODING));
        assert!(!headers.contains_key("session-id"));
        assert!(!headers.contains_key("thread-id"));
        assert!(attempt.preserves_affinity_on_failure());
        assert!(!attempt.successful_response_is_sse());
        assert!(!attempt.changes_request_body());
        assert_eq!(
            attempt
                .request_interface(RequestProtocol::NonStream)
                .unwrap(),
            RequestInterface::StandaloneWebSearch
        );
    }

    #[test]
    fn image_generation_preserves_json_and_uses_image_specific_target_and_headers() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(3),
            ApiOperation::ImagesGeneration,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();
        let original =
            Bytes::from_static(br#"{ "model" : "gpt-image-2", "prompt" : "a red fox" }"#);
        assert_eq!(
            attempt
                .adapt_body(original.clone(), RequestProtocol::NonStream)
                .unwrap(),
            original
        );
        assert_eq!(
            attempt
                .adapt_body(
                    Bytes::from_static(br#"{"model":"gpt-image-2","stream":true}"#),
                    RequestProtocol::Sse,
                )
                .unwrap_err(),
            CodexAttemptError::ImageStreamingUnsupported
        );

        let channel = CompiledChannel::new(
            Uuid::from_u128(3),
            Uuid::from_u128(4),
            crate::domain::ApiFormat::OpenAiImages,
            Url::parse("https://chatgpt.example/backend-api/codex").unwrap(),
            100,
            crate::domain::UpstreamAuth::None,
            std::collections::HashSet::new(),
        );
        let target = attempt
            .upstream_url(
                &channel,
                &"/v1/images/generations?trace=1".parse::<Uri>().unwrap(),
            )
            .unwrap();
        assert_eq!(
            target.as_str(),
            "https://chatgpt.example/backend-api/codex/images/generations?trace=1"
        );

        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_static("client-session"));
        headers.insert("thread-id", HeaderValue::from_static("client-thread"));
        headers.insert(
            "x-client-request-id",
            HeaderValue::from_static("client-request"),
        );
        headers.insert(
            "x-codex-image-turn-id",
            HeaderValue::from_static("client-controlled"),
        );
        attempt
            .inject_headers(&mut headers, RequestProtocol::NonStream)
            .unwrap();

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-token")
        );
        assert_eq!(
            headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok()),
            Some("account-123")
        );
        for header in [
            "x-codex-beta-features",
            "x-codex-routing-hint",
            "x-codex-turn-state",
            "x-openai-internal-codex-responses-lite",
        ] {
            assert!(
                !headers.contains_key(header),
                "{header} must not be generated"
            );
        }
        assert_eq!(
            headers.get(ACCEPT).and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert!(!headers.contains_key(CONTENT_ENCODING));
        assert!(!headers.contains_key("session-id"));
        assert!(!headers.contains_key("thread-id"));
        assert!(!headers.contains_key("x-client-request-id"));
        let turn_id = headers
            .get("x-codex-image-turn-id")
            .and_then(|value| value.to_str().ok())
            .unwrap();
        assert_ne!(turn_id, "client-controlled");
        assert!(Uuid::parse_str(turn_id).is_ok());
        assert_eq!(attempt.credential_id(), Uuid::from_u128(1));
        assert!(!attempt.preserves_affinity_on_failure());
        assert!(!attempt.successful_response_is_sse());
    }

    #[test]
    fn websocket_requests_preserve_incremental_state_and_use_handshake_headers() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            ApiOperation::Responses,
            false,
            &HeaderMap::new(),
            None,
            default_outbound_identity(),
        )
        .unwrap();
        let body = attempt
            .adapt_body(
                Bytes::from_static(
                    br#"{"type":"response.create","model":"gpt-5-codex","stream":true,"store":false,"previous_response_id":"resp_1","generate":false,"client_metadata":{"session_id":"session"}}"#,
                ),
                RequestProtocol::WebSocket,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["previous_response_id"], "resp_1");
        assert_eq!(value["stream"], true);
        assert_eq!(value["store"], false);
        assert_eq!(value["generate"], false);
        assert_eq!(value["client_metadata"]["session_id"], "session");

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        attempt
            .inject_headers(&mut headers, RequestProtocol::WebSocket)
            .unwrap();

        assert!(!headers.contains_key(ACCEPT));
        assert!(!headers.contains_key(ACCEPT_ENCODING));
        assert!(!headers.contains_key(CONTENT_ENCODING));
        assert!(!headers.contains_key(CONTENT_TYPE));
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-token")
        );
        assert_eq!(
            headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok()),
            Some("account-123")
        );
        for header in [
            "x-codex-beta-features",
            "x-codex-routing-hint",
            "x-codex-turn-state",
            "x-openai-internal-codex-responses-lite",
        ] {
            assert!(
                !headers.contains_key(header),
                "{header} must not be generated"
            );
        }
    }

    #[test]
    fn draining_preparation_requires_an_affinity_hit() {
        let now = Utc::now();
        let runtime = CodexCredentialRuntime::new();
        let mut record = runtime_record(now);
        record.runtime_status = "draining".into();
        runtime.replace(vec![record]);

        assert!(matches!(
            PreparedCodexAttempt::prepare(
                &runtime,
                Uuid::from_u128(1),
                ApiOperation::Responses,
                false,
                &HeaderMap::new(),
                Some([1; 32]),
                default_outbound_identity(),
            ),
            Err(CodexCredentialUnavailable::Draining)
        ));
        assert!(
            PreparedCodexAttempt::prepare(
                &runtime,
                Uuid::from_u128(1),
                ApiOperation::Responses,
                true,
                &HeaderMap::new(),
                Some([1; 32]),
                default_outbound_identity(),
            )
            .is_ok()
        );
    }

    fn runtime_record(now: chrono::DateTime<Utc>) -> CodexCredentialRecord {
        CodexCredentialRecord {
            channel_id: Uuid::from_u128(1),
            channel_group_id: Uuid::from_u128(2),
            connector_pool_id: Uuid::from_u128(2),
            projection_channel_ids: vec![Uuid::from_u128(1), Uuid::from_u128(3)],
            label: "credential".into(),
            email: None,
            account_id: Some("account-123".into()),
            user_id: Some("user-123".into()),
            plan_type: Some("plus".into()),
            is_fedramp: false,
            id_token: "id-token".into(),
            access_token: "access-token".into(),
            refresh_token: "refresh-token".into(),
            access_token_expires_at: None,
            last_refreshed_at: now,
            refresh_generation: 7,
            reauth_required: false,
            quota_threshold_percent: 95,
            runtime_status: "active".into(),
            quota_allowed: Some(true),
            quota_limit_reached: Some(false),
            primary_used_percent: Some(10),
            primary_window_seconds: Some(10_800),
            primary_reset_at: None,
            secondary_used_percent: None,
            secondary_window_seconds: None,
            secondary_reset_at: None,
            quota_reset_credits_available: None,
            quota_checked_at: Some(now),
            last_error_code: None,
            last_error_summary: None,
            proxy_id: None,
            weight: 100,
            enabled: true,
            available_models: vec!["gpt-5-codex".into()],
            created_at: now,
            updated_at: now,
        }
    }
}
