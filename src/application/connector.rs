//! In-process upstream connector registry and per-attempt dispatch.

use axum::http::{HeaderMap, HeaderValue, Uri, header::AUTHORIZATION};
use bytes::Bytes;
use reqwest::{StatusCode, Url};

use crate::domain::{ApiOperation, CompiledChannel, ConnectorKind, RequestProtocol, UpstreamAuth};

use super::codex::{
    CodexAttemptError, CodexConnectorService, CodexCredentialUnavailable, PreparedCodexAttempt,
};
use super::request_body::{ImageEditBodyError, PreparedRequestBody, ReplayableRequestBody};

#[derive(Clone, Default)]
pub struct UpstreamConnectorRegistry {
    codex: Option<CodexConnectorService>,
}

impl UpstreamConnectorRegistry {
    #[must_use]
    pub fn with_codex(mut self, service: CodexConnectorService) -> Self {
        self.codex = Some(service);
        self
    }

    pub(crate) fn prepare(
        &self,
        channel: &CompiledChannel,
        api_operation: ApiOperation,
        affinity_cache_hit: bool,
        client_headers: &HeaderMap,
        affinity_hash: Option<[u8; 32]>,
    ) -> Result<PreparedUpstreamAttempt, ConnectorUnavailable> {
        match channel.connector_kind() {
            ConnectorKind::OpenAiCompatible => Ok(PreparedUpstreamAttempt::OpenAiCompatible),
            ConnectorKind::CodexOauth => {
                let service = self.codex.as_ref().ok_or(ConnectorUnavailable::Missing)?;
                let attempt = PreparedCodexAttempt::prepare(
                    &service.runtime(),
                    channel.id(),
                    api_operation,
                    affinity_cache_hit,
                    client_headers,
                    affinity_hash,
                )
                .map_err(ConnectorUnavailable::Codex)?;
                Ok(PreparedUpstreamAttempt::Codex {
                    attempt,
                    service: service.clone(),
                })
            }
        }
    }
}

pub(crate) enum PreparedUpstreamAttempt {
    OpenAiCompatible,
    Codex {
        attempt: PreparedCodexAttempt,
        service: CodexConnectorService,
    },
}

impl PreparedUpstreamAttempt {
    pub(crate) async fn adapt_body(
        &self,
        body: PreparedRequestBody,
        request_protocol: RequestProtocol,
    ) -> Result<ReplayableRequestBody, ConnectorAttemptError> {
        match self {
            Self::OpenAiCompatible => body
                .into_openai_replayable()
                .await
                .map_err(ConnectorAttemptError::RequestBody),
            Self::Codex { attempt, .. } => match body {
                PreparedRequestBody::Json(body) => self
                    .adapt_json_body(body, request_protocol)
                    .map(ReplayableRequestBody::Memory),
                PreparedRequestBody::ImageEdit(body) if attempt.is_image_edit() => body
                    .to_codex_json()
                    .await
                    .map_err(ConnectorAttemptError::RequestBody),
                PreparedRequestBody::ImageEdit(_) => Err(ConnectorAttemptError::from(
                    CodexAttemptError::UnsupportedOperation,
                )),
            },
        }
    }

    pub(crate) fn adapt_json_body(
        &self,
        body: Bytes,
        request_protocol: RequestProtocol,
    ) -> Result<Bytes, ConnectorAttemptError> {
        match self {
            Self::OpenAiCompatible => Ok(body),
            Self::Codex { attempt, .. } => attempt
                .adapt_body(body, request_protocol)
                .map_err(ConnectorAttemptError::from),
        }
    }

    pub(crate) fn upstream_url(
        &self,
        channel: &CompiledChannel,
        uri: &Uri,
    ) -> Result<Url, ConnectorAttemptError> {
        match self {
            Self::OpenAiCompatible => standard_upstream_url(channel, uri),
            Self::Codex { attempt, .. } => attempt
                .upstream_url(channel, uri)
                .map_err(ConnectorAttemptError::from),
        }
    }

    pub(crate) fn inject_headers(
        &self,
        headers: &mut HeaderMap,
        channel: &CompiledChannel,
        request_protocol: RequestProtocol,
    ) -> Result<(), ConnectorAttemptError> {
        match self {
            Self::OpenAiCompatible => inject_standard_auth(headers, channel),
            Self::Codex { attempt, .. } => attempt
                .inject_headers(headers, request_protocol)
                .map_err(ConnectorAttemptError::from),
        }
    }

    #[must_use]
    pub(crate) const fn allows_automatic_retry(&self) -> bool {
        matches!(self, Self::OpenAiCompatible)
    }

    #[must_use]
    pub(crate) fn preserves_affinity_on_failure(&self) -> bool {
        match self {
            Self::OpenAiCompatible => false,
            Self::Codex { attempt, .. } => attempt.preserves_affinity_on_failure(),
        }
    }

    /// Codex's successful Responses endpoint is an SSE protocol even if an
    /// intermediary omits or rewrites the response Content-Type.
    #[must_use]
    pub(crate) fn successful_response_is_sse(&self) -> bool {
        match self {
            Self::OpenAiCompatible => false,
            Self::Codex { attempt, .. } => attempt.successful_response_is_sse(),
        }
    }

    #[must_use]
    pub(crate) fn changes_request_body(&self) -> bool {
        match self {
            Self::OpenAiCompatible => false,
            Self::Codex { attempt, .. } => attempt.changes_request_body(),
        }
    }

    pub(crate) fn observe_response(&self, status: StatusCode) {
        if status != StatusCode::UNAUTHORIZED {
            return;
        }
        if let Self::Codex { attempt, service } = self {
            let service = service.clone();
            let credential_id = attempt.credential_id();
            let refresh_generation = attempt.refresh_generation();
            tokio::spawn(async move {
                service
                    .report_unauthorized(credential_id, refresh_generation)
                    .await;
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectorUnavailable {
    Missing,
    Codex(CodexCredentialUnavailable),
}

impl ConnectorUnavailable {
    #[must_use]
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Missing => "upstream_connector_missing",
            Self::Codex(CodexCredentialUnavailable::Missing) => "codex_credential_missing",
            Self::Codex(CodexCredentialUnavailable::Draining) => "codex_credential_draining",
            Self::Codex(CodexCredentialUnavailable::Unavailable) => "codex_credential_unavailable",
            Self::Codex(CodexCredentialUnavailable::Disabled) => "codex_credential_disabled",
            Self::Codex(CodexCredentialUnavailable::Expired) => "codex_credential_expired",
        }
    }

    #[must_use]
    pub(crate) const fn sticky_code(self) -> &'static str {
        match self {
            Self::Codex(_) => "codex_sticky_credential_unavailable",
            Self::Missing => "upstream_connector_sticky_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectorAttemptError {
    ClientRequest {
        message: &'static str,
        param: &'static str,
        code: &'static str,
    },
    RequestBody(ImageEditBodyError),
    InvalidTarget,
    InvalidCredentials,
}

impl From<CodexAttemptError> for ConnectorAttemptError {
    fn from(value: CodexAttemptError) -> Self {
        match value {
            CodexAttemptError::StreamingRequired => Self::ClientRequest {
                message: "Codex OAuth channels currently require `stream: true`.",
                param: "stream",
                code: "codex_streaming_required",
            },
            CodexAttemptError::PreviousResponseUnsupported => Self::ClientRequest {
                message: "Codex OAuth HTTP requests do not support `previous_response_id`.",
                param: "previous_response_id",
                code: "codex_previous_response_unsupported",
            },
            CodexAttemptError::ImageStreamingUnsupported => Self::ClientRequest {
                message: "Codex OAuth Images generation does not support streaming.",
                param: "stream",
                code: "image_streaming_unsupported",
            },
            CodexAttemptError::UnsupportedOperation => Self::InvalidTarget,
            CodexAttemptError::InvalidRequestBody => Self::ClientRequest {
                message: "Request body must be a JSON object.",
                param: "body",
                code: "invalid_request",
            },
            CodexAttemptError::InvalidTarget => Self::InvalidTarget,
            CodexAttemptError::InvalidCredentials => Self::InvalidCredentials,
        }
    }
}

pub(crate) fn standard_upstream_url(
    channel: &CompiledChannel,
    uri: &Uri,
) -> Result<Url, ConnectorAttemptError> {
    let base = channel.base_url().as_str().trim_end_matches('/');
    let query = uri
        .query()
        .map_or_else(String::new, |query| format!("?{query}"));
    Url::parse(&format!("{base}{}{query}", uri.path()))
        .map_err(|_| ConnectorAttemptError::InvalidTarget)
}

pub(crate) fn inject_standard_auth(
    headers: &mut HeaderMap,
    channel: &CompiledChannel,
) -> Result<(), ConnectorAttemptError> {
    match channel.upstream_auth() {
        UpstreamAuth::None => {}
        UpstreamAuth::Bearer(token) => {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| ConnectorAttemptError::InvalidCredentials)?,
            );
        }
        UpstreamAuth::Header { name, value } => {
            headers.insert(
                name.clone(),
                HeaderValue::from_str(value)
                    .map_err(|_| ConnectorAttemptError::InvalidCredentials)?,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc};

    use reqwest::header::HeaderName;
    use uuid::Uuid;

    use crate::domain::{ApiFormat, CompiledChannel};

    use super::*;

    #[test]
    fn standard_connector_preserves_path_and_injects_configured_auth() {
        let channel = CompiledChannel::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiChatCompletions,
            Url::parse("https://example.test/base").unwrap(),
            1,
            UpstreamAuth::Header {
                name: HeaderName::from_static("x-api-key"),
                value: Arc::from("upstream-secret"),
            },
            HashSet::new(),
        );
        let attempt = PreparedUpstreamAttempt::OpenAiCompatible;
        let target = attempt
            .upstream_url(
                &channel,
                &"/v1/chat/completions?trace=1".parse::<Uri>().unwrap(),
            )
            .unwrap();
        assert_eq!(
            target.as_str(),
            "https://example.test/base/v1/chat/completions?trace=1"
        );

        let mut headers = HeaderMap::new();
        attempt
            .inject_headers(&mut headers, &channel, RequestProtocol::NonStream)
            .unwrap();
        assert_eq!(headers.get("x-api-key").unwrap(), "upstream-secret");
        assert!(attempt.allows_automatic_retry());
    }
}
