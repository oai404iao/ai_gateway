//! Built-in general connector and external connector attempt dispatch.

use axum::http::{HeaderMap, HeaderValue, Uri, header::AUTHORIZATION};
use bytes::Bytes;
use reqwest::{StatusCode, Url};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

use crate::connector_plugins::{ConnectorPlugins, Plugin};
use crate::domain::{
    ApiOperation, CodexRequestMetadataSettings, CompiledChannel, ConnectorKind, RequestProtocol,
    UpstreamAuth,
};
use crate::request_policy::{
    CodexRequestMetadata, RequestInterface, RequestPolicyError, RequestPolicyLayer,
    apply_json_body_policy, filter_codex_headers, normalize_codex_fingerprints_in_headers,
    normalize_codex_fingerprints_in_json,
};

use super::codex::{
    CodexAttemptError, CodexConnectorService, CodexCredentialUnavailable, PreparedCodexAttempt,
};
use super::request_body::{ImageEditBodyError, PreparedRequestBody, ReplayableRequestBody};

#[derive(Clone, Default)]
pub struct UpstreamConnectorRegistry {
    codex: Option<CodexConnectorService>,
    plugins: ConnectorPlugins,
}

impl UpstreamConnectorRegistry {
    #[must_use]
    pub fn with_plugins(mut self, plugins: ConnectorPlugins) -> Self {
        self.plugins = plugins;
        self
    }

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
        codex_settings: &CodexRequestMetadataSettings,
    ) -> Result<PreparedUpstreamAttempt, ConnectorUnavailable> {
        match channel.connector_kind() {
            ConnectorKind::OpenAiCompatible => Ok(PreparedUpstreamAttempt::OpenAiCompatible),
            ConnectorKind::CodexOauth => {
                let service = self.codex.as_ref().ok_or(ConnectorUnavailable::Missing)?;
                let credential_id = channel
                    .credential_id()
                    .ok_or(ConnectorUnavailable::Missing)?;
                let attempt = PreparedCodexAttempt::prepare(
                    &service.runtime(),
                    service.plugin().ok_or(ConnectorUnavailable::Missing)?,
                    credential_id,
                    api_operation,
                    affinity_cache_hit,
                    client_headers,
                    affinity_hash,
                    codex_settings.outbound_identity().clone(),
                )
                .map_err(ConnectorUnavailable::Codex)?;
                let request_metadata = attempt.request_metadata(codex_settings).map(Box::new);
                Ok(PreparedUpstreamAttempt::Codex {
                    attempt: Box::new(attempt),
                    request_metadata,
                    service: service.clone(),
                })
            }
            ConnectorKind::Plugin(id) => {
                let plugin = self
                    .plugins
                    .get(id.as_str())
                    .ok_or(ConnectorUnavailable::Missing)?;
                if !plugin
                    .manifest()
                    .operations
                    .iter()
                    .any(|operation| operation == api_operation.as_str())
                {
                    return Err(ConnectorUnavailable::Missing);
                }
                let capabilities = plugin
                    .call(
                        "attempt.capabilities",
                        &json!({"operation":api_operation}),
                        &[],
                    )
                    .map_err(|_| ConnectorUnavailable::Missing)?;
                let successful_response_is_sse =
                    capabilities.metadata["successful_response_is_sse"]
                        .as_bool()
                        .ok_or(ConnectorUnavailable::Missing)?;
                Ok(PreparedUpstreamAttempt::External {
                    plugin,
                    operation: api_operation,
                    successful_response_is_sse,
                    image_content_type: OnceLock::new(),
                })
            }
        }
    }

    #[must_use]
    pub(crate) fn can_attempt_responses_websocket(&self, channel: &CompiledChannel) -> bool {
        match channel.connector_kind() {
            ConnectorKind::OpenAiCompatible => true,
            ConnectorKind::CodexOauth => self.codex.as_ref().is_some_and(|service| {
                // The model and request-specific affinity are unavailable during
                // Upgrade, so draining credentials remain potential candidates.
                service.plugin().is_some()
                    && channel
                        .credential_id()
                        .is_some_and(|id| service.runtime().credential(id, true).is_ok())
            }),
            ConnectorKind::Plugin(id) => self.plugins.get(id.as_str()).is_some_and(|p| {
                p.manifest()
                    .operations
                    .iter()
                    .any(|operation| operation == "responses-ws")
            }),
        }
    }
}

pub(crate) enum PreparedUpstreamAttempt {
    OpenAiCompatible,
    External {
        plugin: Arc<Plugin>,
        operation: ApiOperation,
        successful_response_is_sse: bool,
        image_content_type: OnceLock<HeaderValue>,
    },
    Codex {
        attempt: Box<PreparedCodexAttempt>,
        request_metadata: Option<Box<CodexRequestMetadata>>,
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
            Self::External {
                plugin,
                image_content_type,
                ..
            } => match body {
                PreparedRequestBody::Json(body) => self
                    .adapt_json_body(body, request_protocol)
                    .map(ReplayableRequestBody::Memory),
                PreparedRequestBody::ImageEdit(body) => {
                    let (body, content_type) = body
                        .to_plugin_body(plugin)
                        .await
                        .map_err(ConnectorAttemptError::RequestBody)?;
                    image_content_type
                        .set(content_type)
                        .map_err(|_| ConnectorAttemptError::InvalidTarget)?;
                    Ok(body)
                }
            },
            Self::Codex { attempt, .. } => match body {
                PreparedRequestBody::Json(body) => self
                    .adapt_json_body(body, request_protocol)
                    .map(ReplayableRequestBody::Memory),
                PreparedRequestBody::ImageEdit(body) if attempt.is_image_edit() => {
                    let (body, _) = PreparedRequestBody::ImageEdit(body)
                        .apply_policy(RequestPolicyLayer::CodexOauth, RequestInterface::ImagesEdit)
                        .map_err(ConnectorAttemptError::RequestPolicy)?;
                    body.image_edit()
                        .expect("Codex Images edit policy preserves the body kind")
                        .to_plugin_body(attempt.plugin())
                        .await
                        .map_err(ConnectorAttemptError::RequestBody)
                        .and_then(|(body, content_type)| {
                            if content_type != "application/json" {
                                return Err(ConnectorAttemptError::InvalidTarget);
                            }
                            Ok(body)
                        })
                }
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
            Self::External {
                plugin, operation, ..
            } => {
                let output = plugin
                    .call(
                        "attempt.body",
                        &json!({"operation":operation,"protocol":request_protocol}),
                        &body,
                    )
                    .map_err(|_| ConnectorAttemptError::InvalidTarget)?;
                validate_plugin_body(&body, &output.body)?;
                Ok(Bytes::from(output.body))
            }
            Self::Codex {
                attempt,
                request_metadata,
                ..
            } => {
                let interface = attempt
                    .request_interface(request_protocol)
                    .map_err(ConnectorAttemptError::from)?;
                let body = apply_json_body_policy(RequestPolicyLayer::CodexOauth, interface, body)
                    .map_err(ConnectorAttemptError::RequestPolicy)?
                    .body;
                let body = if let Some(metadata) = request_metadata {
                    normalize_codex_fingerprints_in_json(interface, body, metadata)
                        .map_err(ConnectorAttemptError::RequestPolicy)?
                        .body
                } else {
                    body
                };
                let adapted = attempt
                    .adapt_body(body.clone(), request_protocol)
                    .map_err(ConnectorAttemptError::from)?;
                validate_plugin_body(&body, &adapted)?;
                let adapted =
                    apply_json_body_policy(RequestPolicyLayer::CodexOauth, interface, adapted)
                        .map_err(ConnectorAttemptError::RequestPolicy)?
                        .body;
                if let Some(metadata) = request_metadata {
                    normalize_codex_fingerprints_in_json(interface, adapted, metadata)
                        .map(|out| out.body)
                        .map_err(ConnectorAttemptError::RequestPolicy)
                } else {
                    Ok(adapted)
                }
            }
        }
    }

    pub(crate) fn upstream_url(
        &self,
        channel: &CompiledChannel,
        uri: &Uri,
    ) -> Result<Url, ConnectorAttemptError> {
        match self {
            Self::OpenAiCompatible => standard_upstream_url(channel, uri),
            Self::External {
                plugin, operation, ..
            } => {
                let output = plugin.call("attempt.target", &json!({
                    "operation":operation,"base_url":channel.base_url().as_str(),"query":uri.query(),
                    "path":uri.path(),
                }), &[]).map_err(|_| ConnectorAttemptError::InvalidTarget)?;
                validate_plugin_target(
                    channel,
                    output.metadata["url"]
                        .as_str()
                        .ok_or(ConnectorAttemptError::InvalidTarget)?,
                )
            }
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
            Self::External {
                plugin,
                operation,
                image_content_type,
                ..
            } => {
                let output = plugin.call("attempt.headers", &json!({
                    "operation":operation,"protocol":request_protocol,"headers":plugin_header_metadata(headers),
                }), &[]).map_err(|_| ConnectorAttemptError::InvalidCredentials)?;
                apply_plugin_headers(headers, &output.metadata)?;
                if let Some(content_type) = image_content_type.get() {
                    headers.insert(axum::http::header::CONTENT_TYPE, content_type.clone());
                }
                headers.remove(AUTHORIZATION);
                inject_standard_auth(headers, channel)
            }
            Self::Codex {
                attempt,
                request_metadata,
                ..
            } => {
                let interface = attempt
                    .request_interface(request_protocol)
                    .map_err(ConnectorAttemptError::from)?;
                *headers = filter_codex_headers(interface, headers)
                    .map_err(ConnectorAttemptError::RequestPolicy)?;
                if let Some(metadata) = request_metadata {
                    normalize_codex_fingerprints_in_headers(interface, headers, metadata);
                }
                attempt
                    .inject_headers(headers, request_protocol)
                    .map_err(ConnectorAttemptError::from)
            }
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
            Self::External { .. } => false,
            Self::Codex { attempt, .. } => attempt.preserves_affinity_on_failure(),
        }
    }

    /// Codex's successful Responses endpoint is an SSE protocol even if an
    /// intermediary omits or rewrites the response Content-Type.
    #[must_use]
    pub(crate) fn successful_response_is_sse(&self) -> bool {
        match self {
            Self::OpenAiCompatible => false,
            Self::External {
                successful_response_is_sse,
                ..
            } => *successful_response_is_sse,
            Self::Codex { attempt, .. } => attempt.successful_response_is_sse(),
        }
    }

    #[must_use]
    pub(crate) fn changes_request_body(&self) -> bool {
        match self {
            Self::OpenAiCompatible => false,
            Self::External { .. } => true,
            Self::Codex { attempt, .. } => attempt.changes_request_body(),
        }
    }

    pub(crate) fn observe_response(&self, status: StatusCode) {
        if status != StatusCode::UNAUTHORIZED {
            return;
        }
        if let Self::Codex {
            attempt, service, ..
        } = self
        {
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

fn validate_plugin_body(before: &[u8], after: &[u8]) -> Result<(), ConnectorAttemptError> {
    let before: Value =
        serde_json::from_slice(before).map_err(|_| ConnectorAttemptError::InvalidTarget)?;
    let after: Value =
        serde_json::from_slice(after).map_err(|_| ConnectorAttemptError::InvalidTarget)?;
    // Plugins cannot replace the selected wire model or restore filtered billing metadata.
    if !after.is_object()
        || before.get("model") != after.get("model")
        || before.get("service_tier") != after.get("service_tier")
    {
        return Err(ConnectorAttemptError::InvalidTarget);
    }
    Ok(())
}

pub(crate) fn plugin_header_metadata(headers: &HeaderMap) -> Value {
    let values: serde_json::Map<String, Value> = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.to_string(), Value::String(v.into())))
        })
        .collect();
    Value::Object(values)
}

pub(crate) fn apply_plugin_headers(
    headers: &mut HeaderMap,
    plan: &Value,
) -> Result<(), ConnectorAttemptError> {
    let remove = plan["remove"]
        .as_array()
        .ok_or(ConnectorAttemptError::InvalidCredentials)?;
    let set = plan["set"]
        .as_object()
        .ok_or(ConnectorAttemptError::InvalidCredentials)?;
    let mut updated = headers.clone();
    let mut names = std::collections::HashSet::new();
    for name in remove {
        let name = name
            .as_str()
            .ok_or(ConnectorAttemptError::InvalidCredentials)?;
        let name = axum::http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ConnectorAttemptError::InvalidCredentials)?;
        updated.remove(name);
    }
    for (name, value) in set {
        let name = axum::http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ConnectorAttemptError::InvalidCredentials)?;
        if !names.insert(name.clone())
            || crate::request_policy::connector_header_is_forbidden(&name)
        {
            return Err(ConnectorAttemptError::InvalidCredentials);
        }
        let value = HeaderValue::from_str(
            value
                .as_str()
                .ok_or(ConnectorAttemptError::InvalidCredentials)?,
        )
        .map_err(|_| ConnectorAttemptError::InvalidCredentials)?;
        updated.insert(name, value);
    }
    *headers = updated;
    Ok(())
}

pub(crate) fn validate_plugin_target(
    channel: &CompiledChannel,
    value: &str,
) -> Result<Url, ConnectorAttemptError> {
    let target = Url::parse(value).map_err(|_| ConnectorAttemptError::InvalidTarget)?;
    let base = channel.base_url();
    let base_path = base.path().trim_end_matches('/');
    if target.origin() != base.origin()
        || target.username() != base.username()
        || target.password() != base.password()
        || target.fragment().is_some()
        || !(target.path() == base_path || target.path().starts_with(&format!("{base_path}/")))
    {
        return Err(ConnectorAttemptError::InvalidTarget);
    }
    Ok(target)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConnectorAttemptError {
    ClientRequest {
        message: &'static str,
        param: &'static str,
        code: &'static str,
    },
    RequestBody(ImageEditBodyError),
    RequestPolicy(RequestPolicyError),
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
            CodexAttemptError::SearchStreamingUnsupported => Self::ClientRequest {
                message: "Standalone web search does not support streaming.",
                param: "stream",
                code: "standalone_web_search_streaming_unsupported",
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
pub(crate) mod tests {
    use std::{collections::HashSet, sync::Arc};

    use reqwest::header::HeaderName;
    use uuid::Uuid;

    use rust_decimal::Decimal;

    use crate::domain::{
        ApiFormat, CompiledChannel, CompiledChannelUpstreamPolicy, ConnectorKind,
        RequestCompression,
    };

    use super::*;

    #[test]
    fn external_body_cannot_change_routing_or_restore_fast_mode() {
        let before = br#"{"model":"selected","input":"hello"}"#;
        assert!(validate_plugin_body(before, before).is_ok());
        assert!(validate_plugin_body(before, br#"{"model":"other","input":"hello"}"#).is_err());
        assert!(
            validate_plugin_body(before, br#"{"model":"selected","service_tier":"priority"}"#)
                .is_err()
        );
        assert!(
            validate_plugin_body(before, br#"{"model":"selected","service_tier":null}"#).is_err()
        );
    }

    #[test]
    fn plugin_header_plan_rejects_transport_metadata_atomically() {
        for name in [
            "host",
            "content-length",
            "connection",
            "transfer-encoding",
            "forwarded",
            "x-forwarded-for",
            "sec-websocket-key",
            "cookie",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-existing", HeaderValue::from_static("keep"));
            let plan = json!({"remove":["x-existing"],"set":{name:"injected"}});
            assert!(apply_plugin_headers(&mut headers, &plan).is_err(), "{name}");
            assert_eq!(headers["x-existing"], "keep");
            assert_eq!(headers.len(), 1);
        }
        let mut headers = HeaderMap::new();
        assert!(
            apply_plugin_headers(
                &mut headers,
                &json!({"remove":[],"set":{"x-provider":"bad\r\nheader"}})
            )
            .is_err()
        );
        assert!(headers.is_empty());
        for set in [
            json!({"Authorization":"Bearer other", "authorization":"Bearer expected"}),
            json!({"ChatGPT-Account-ID":"other", "chatgpt-account-id":"expected"}),
            json!({"X-OpenAI-Fedramp":"false", "x-openai-fedramp":"true"}),
        ] {
            assert!(apply_plugin_headers(&mut headers, &json!({"remove":[],"set":set})).is_err());
            assert!(headers.is_empty());
        }
    }

    #[cfg(target_os = "linux")]
    fn fixture(
        empty_commands: bool,
    ) -> Result<ConnectorPlugins, crate::connector_plugins::PluginError> {
        fixture_with_mode(empty_commands, 0)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn fixture_with_mode(
        empty_commands: bool,
        body_mode: u8,
    ) -> Result<ConnectorPlugins, crate::connector_plugins::PluginError> {
        use sha2::{Digest, Sha256};
        use std::path::PathBuf;

        let codex_path = PathBuf::from(
            std::env::var_os("AI_GATEWAY_TEST_CODEX_PLUGIN")
                .expect("run scripts/prepare-connector-tests.sh and export its plugin path"),
        );
        let filename = if empty_commands {
            "fixture-empty.so".to_owned()
        } else {
            format!("fixture-{body_mode}.so")
        };
        let path = codex_path
            .parent()
            .expect("the test Codex plugin path must have a parent directory")
            .join(filename);
        let bytes = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "missing prepared connector fixture {}: {error}; rerun scripts/prepare-connector-tests.sh",
                path.display()
            )
        });
        let sha256 = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        ConnectorPlugins::load(&[crate::connector_plugins::PluginConfig {
            id: "fixture".into(),
            path,
            sha256,
        }])
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn generic_external_connector_cannot_add_authorization_to_other_auth_modes() {
        let registry = UpstreamConnectorRegistry::default().with_plugins(fixture(false).unwrap());
        for auth in [
            UpstreamAuth::None,
            UpstreamAuth::Header {
                name: HeaderName::from_static("x-provider"),
                value: Arc::from("host-selected-secret"),
            },
        ] {
            let custom_header = matches!(auth, UpstreamAuth::Header { .. });
            let channel = CompiledChannel::new_with_connector_policy_automation_and_billing(
                Uuid::new_v4(),
                Uuid::new_v4(),
                ApiFormat::OpenAiResponses,
                ConnectorKind::parse("fixture").unwrap(),
                RequestCompression::Default,
                Url::parse("https://upstream.test/base").unwrap(),
                Decimal::ONE,
                auth,
                HashSet::new(),
                true,
                false,
                false,
                false,
                None,
                CompiledChannelUpstreamPolicy::transparent(ApiFormat::OpenAiResponses),
            );
            let attempt = registry
                .prepare(
                    &channel,
                    ApiOperation::Responses,
                    false,
                    &HeaderMap::new(),
                    None,
                    &CodexRequestMetadataSettings::default(),
                )
                .unwrap();
            let mut headers = HeaderMap::new();
            attempt
                .inject_headers(&mut headers, &channel, RequestProtocol::Sse)
                .unwrap();
            assert!(!headers.contains_key(AUTHORIZATION));
            assert_eq!(
                headers["x-provider"],
                if custom_header {
                    "host-selected-secret"
                } else {
                    "fixture"
                }
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn generic_external_connector_dispatches_through_native_abi_with_host_auth() {
        let plugins = fixture(false).unwrap();
        let registry = UpstreamConnectorRegistry::default().with_plugins(plugins);
        let channel = CompiledChannel::new_with_connector_policy_automation_and_billing(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiResponses,
            ConnectorKind::parse("fixture").unwrap(),
            RequestCompression::Default,
            Url::parse("https://upstream.test/base").unwrap(),
            Decimal::ONE,
            UpstreamAuth::Bearer(Arc::from("host-owned-secret")),
            HashSet::new(),
            true,
            false,
            false,
            false,
            None,
            CompiledChannelUpstreamPolicy::transparent(ApiFormat::OpenAiResponses),
        );
        let attempt = registry
            .prepare(
                &channel,
                ApiOperation::Responses,
                false,
                &HeaderMap::new(),
                None,
                &CodexRequestMetadataSettings::default(),
            )
            .unwrap();
        let body = Bytes::from_static(br#"{ "model":"selected","input":"hello","stream":true }"#);
        assert_eq!(
            attempt
                .adapt_json_body(body.clone(), RequestProtocol::Sse)
                .unwrap(),
            body
        );
        assert_eq!(
            attempt
                .upstream_url(&channel, &"/v1/responses".parse().unwrap())
                .unwrap()
                .as_str(),
            "https://upstream.test/base/responses"
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-remove", HeaderValue::from_static("old"));
        attempt
            .inject_headers(&mut headers, &channel, RequestProtocol::Sse)
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer host-owned-secret");
        assert_eq!(headers["x-provider"], "fixture");
        assert!(!headers.contains_key("x-remove"));
        assert!(attempt.successful_response_is_sse());
        assert!(!attempt.allows_automatic_retry());
        assert!(registry.can_attempt_responses_websocket(&channel));
        let image_attempt = registry
            .prepare(
                &channel,
                ApiOperation::ImagesEdit,
                false,
                &HeaderMap::new(),
                None,
                &CodexRequestMetadataSettings::default(),
            )
            .unwrap();
        let PreparedUpstreamAttempt::External {
            image_content_type, ..
        } = &image_attempt
        else {
            panic!("external attempt");
        };
        image_content_type
            .set(HeaderValue::from_static(
                "multipart/form-data; boundary=retained",
            ))
            .unwrap();
        image_attempt
            .inject_headers(&mut headers, &channel, RequestProtocol::NonStream)
            .unwrap();
        assert_eq!(
            headers[axum::http::header::CONTENT_TYPE],
            "multipart/form-data; boundary=retained"
        );
        assert!(
            registry
                .prepare(
                    &channel,
                    ApiOperation::ImagesGeneration,
                    false,
                    &HeaderMap::new(),
                    None,
                    &CodexRequestMetadataSettings::default()
                )
                .is_err()
        );
        assert!(matches!(
            fixture(true),
            Err(crate::connector_plugins::PluginError::Manifest)
        ));
        for invalid in [
            "https://attacker.test/base/responses",
            "https://upstream.test/elsewhere",
            "https://upstream.test/base/../elsewhere",
            "https://upstream.test/base/responses#fragment",
            "https://user:password@upstream.test/base/responses",
        ] {
            assert!(
                validate_plugin_target(&channel, invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn standard_connector_preserves_path_and_injects_configured_auth() {
        let channel = CompiledChannel::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiChatCompletions,
            Url::parse("https://example.test/base").unwrap(),
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
        assert!(UpstreamConnectorRegistry::default().can_attempt_responses_websocket(&channel));
    }

    #[test]
    fn websocket_preflight_excludes_a_missing_codex_connector() {
        let channel = CompiledChannel::new_with_connector_policy_automation_and_billing(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiResponses,
            ConnectorKind::CodexOauth,
            RequestCompression::Default,
            Url::parse("https://example.test/backend-api/codex").unwrap(),
            Decimal::ONE,
            UpstreamAuth::None,
            HashSet::new(),
            true,
            false,
            false,
            false,
            None,
            CompiledChannelUpstreamPolicy::transparent(ApiFormat::OpenAiResponses),
        );

        assert!(!UpstreamConnectorRegistry::default().can_attempt_responses_websocket(&channel));
    }
}
