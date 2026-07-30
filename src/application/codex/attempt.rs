use axum::http::{
    HeaderMap, HeaderValue, Uri,
    header::{ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, USER_AGENT},
};
use bytes::Bytes;
use reqwest::Url;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::{CompiledChannel, RequestProtocol};

use super::{
    CODEX_CLIENT_VERSION, CODEX_ORIGINATOR, CodexCredentialRuntime, CodexCredentialUnavailable,
    CompiledCodexCredential, codex_user_agent,
};

#[derive(Clone)]
pub(crate) struct PreparedCodexAttempt {
    credential: std::sync::Arc<CompiledCodexCredential>,
    identity: CodexRequestIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexAttemptError {
    StreamingRequired,
    PreviousResponseUnsupported,
    InvalidRequestBody,
    InvalidTarget,
    InvalidCredentials,
}

impl PreparedCodexAttempt {
    pub(crate) fn prepare(
        runtime: &CodexCredentialRuntime,
        channel_id: Uuid,
        affinity_cache_hit: bool,
        client_headers: &HeaderMap,
        affinity_hash: Option<[u8; 32]>,
    ) -> Result<Self, CodexCredentialUnavailable> {
        Ok(Self {
            credential: runtime.credential(channel_id, affinity_cache_hit)?,
            identity: CodexRequestIdentity::new(client_headers, affinity_hash),
        })
    }

    pub(crate) fn adapt_body(
        &self,
        body: Bytes,
        request_protocol: RequestProtocol,
    ) -> Result<Bytes, CodexAttemptError> {
        if !request_protocol.is_streamed() {
            return Err(CodexAttemptError::StreamingRequired);
        }
        let mut value = serde_json::from_slice::<Value>(&body)
            .map_err(|_| CodexAttemptError::InvalidRequestBody)?;
        let object = value
            .as_object_mut()
            .ok_or(CodexAttemptError::InvalidRequestBody)?;
        if object.get("previous_response_id").is_some_and(|value| {
            !value.is_null() && !matches!(value, Value::String(value) if value.is_empty())
        }) {
            return Err(CodexAttemptError::PreviousResponseUnsupported);
        }
        object.remove("previous_response_id");
        object.insert("store".to_owned(), Value::Bool(false));
        object.insert("stream".to_owned(), Value::Bool(true));
        serde_json::to_vec(&value)
            .map(Bytes::from)
            .map_err(|_| CodexAttemptError::InvalidRequestBody)
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
        Url::parse(&format!("{base}/responses{query}"))
            .map_err(|_| CodexAttemptError::InvalidTarget)
    }

    pub(crate) fn inject_headers(&self, headers: &mut HeaderMap) -> Result<(), CodexAttemptError> {
        let invalid = |_| CodexAttemptError::InvalidCredentials;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.credential.access_token()))
                .map_err(invalid)?,
        );
        headers.insert(
            "ChatGPT-Account-ID",
            HeaderValue::from_str(self.credential.account_id()).map_err(invalid)?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        // The Gateway must inspect Codex's terminal SSE event for usage and
        // completion before clients stop polling the response body.
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&codex_user_agent()).map_err(invalid)?,
        );
        headers.insert("originator", HeaderValue::from_static(CODEX_ORIGINATOR));
        headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));
        headers.insert(
            "session-id",
            HeaderValue::from_str(&self.identity.session_id).map_err(invalid)?,
        );
        headers.insert(
            "thread-id",
            HeaderValue::from_str(&self.identity.thread_id).map_err(invalid)?,
        );
        if self.credential.is_fedramp() {
            headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        } else {
            headers.remove("X-OpenAI-Fedramp");
        }
        if !headers.contains_key("x-client-request-id") {
            headers.insert(
                "x-client-request-id",
                HeaderValue::from_str(&self.identity.thread_id).map_err(invalid)?,
            );
        }
        Ok(())
    }

    pub(crate) fn refresh_generation(&self) -> i64 {
        self.credential.refresh_generation()
    }
}

#[derive(Clone)]
struct CodexRequestIdentity {
    session_id: String,
    thread_id: String,
}

impl CodexRequestIdentity {
    fn new(headers: &HeaderMap, affinity_hash: Option<[u8; 32]>) -> Self {
        let session_id = valid_identity_header(headers, "session-id");
        let thread_id = valid_identity_header(headers, "thread-id");
        match (session_id, thread_id) {
            (Some(session_id), Some(thread_id)) => Self {
                session_id,
                thread_id,
            },
            (Some(session_id), None) => Self {
                thread_id: session_id.clone(),
                session_id,
            },
            (None, Some(thread_id)) => Self {
                session_id: thread_id.clone(),
                thread_id,
            },
            (None, None) => {
                let seed = affinity_hash.unwrap_or_else(random_identity_seed);
                Self {
                    session_id: opaque_uuid(&seed, b"codex-session"),
                    thread_id: opaque_uuid(&seed, b"codex-thread"),
                }
            }
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
    use chrono::Utc;

    use crate::persistence::CodexCredentialRecord;

    use super::*;

    fn runtime() -> CodexCredentialRuntime {
        let now = Utc::now();
        let runtime = CodexCredentialRuntime::new();
        runtime.replace(vec![CodexCredentialRecord {
            channel_id: Uuid::from_u128(1),
            channel_group_id: Uuid::from_u128(2),
            label: "credential".into(),
            email: None,
            account_id: "account-123".into(),
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
    fn request_adapter_forces_streaming_and_disables_storage() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            false,
            &HeaderMap::new(),
            None,
        )
        .unwrap();
        let body = attempt
            .adapt_body(
                Bytes::from_static(br#"{"model":"gpt-5-codex","stream":true,"store":true}"#),
                RequestProtocol::Sse,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["stream"], true);
        assert_eq!(value["store"], false);
        assert_eq!(value["model"], "gpt-5-codex");
    }

    #[test]
    fn request_adapter_rejects_non_streaming_and_previous_response_ids() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            false,
            &HeaderMap::new(),
            None,
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
        assert_eq!(
            attempt
                .adapt_body(
                    Bytes::from_static(
                        br#"{"model":"gpt-5-codex","stream":true,"previous_response_id":"resp_1"}"#,
                    ),
                    RequestProtocol::Sse,
                )
                .unwrap_err(),
            CodexAttemptError::PreviousResponseUnsupported
        );
        let body = attempt
            .adapt_body(
                Bytes::from_static(
                    br#"{"model":"gpt-5-codex","stream":true,"previous_response_id":""}"#,
                ),
                RequestProtocol::Sse,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert!(value.get("previous_response_id").is_none());
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
    fn request_headers_report_the_pinned_codex_client_identity() {
        let attempt = PreparedCodexAttempt::prepare(
            &runtime(),
            Uuid::from_u128(1),
            false,
            &HeaderMap::new(),
            None,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));

        attempt.inject_headers(&mut headers).unwrap();

        assert_eq!(
            headers
                .get(ACCEPT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            Some("identity")
        );
        assert_eq!(
            headers.get("version").and_then(|value| value.to_str().ok()),
            Some(CODEX_CLIENT_VERSION)
        );
        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some(CODEX_ORIGINATOR)
        );
        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(codex_user_agent().as_str())
        );
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
                false,
                &HeaderMap::new(),
                Some([1; 32]),
            ),
            Err(CodexCredentialUnavailable::Draining)
        ));
        assert!(
            PreparedCodexAttempt::prepare(
                &runtime,
                Uuid::from_u128(1),
                true,
                &HeaderMap::new(),
                Some([1; 32]),
            )
            .is_ok()
        );
    }

    fn runtime_record(now: chrono::DateTime<Utc>) -> CodexCredentialRecord {
        CodexCredentialRecord {
            channel_id: Uuid::from_u128(1),
            channel_group_id: Uuid::from_u128(2),
            label: "credential".into(),
            email: None,
            account_id: "account-123".into(),
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
