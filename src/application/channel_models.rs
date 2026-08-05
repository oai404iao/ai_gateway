//! Administrator-triggered discovery of OpenAI-compatible upstream models.

use std::{collections::HashSet, sync::Arc, time::Duration};

use axum::http::{
    HeaderMap, HeaderValue, Method,
    header::{ACCEPT, AUTHORIZATION},
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    domain::{CompiledChannel, UpstreamAuth},
    persistence::ChannelRecord,
    request_policy::strip_explicitly_ignored_client_headers,
    runtime_config::{RuntimeConfig, compile_channel_discovery_target},
    transforms::apply_header_plan,
    upstream::{ResolvedUpstreamPolicy, UpstreamClientRegistry},
};

const MAX_RESPONSE_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_DISCOVERED_MODELS: usize = 10_000;
const MAX_MODEL_ID_BYTES: usize = 1_024;

#[derive(Clone)]
pub struct ChannelModelDiscoveryService {
    runtime: Arc<RuntimeConfig>,
    upstream_clients: Arc<UpstreamClientRegistry>,
}

impl ChannelModelDiscoveryService {
    #[must_use]
    pub fn new(runtime: Arc<RuntimeConfig>, upstream_clients: Arc<UpstreamClientRegistry>) -> Self {
        Self {
            runtime,
            upstream_clients,
        }
    }

    /// Fetches `GET /v1/models` using the draft channel's effective proxy,
    /// timeout, transform-header, and authentication configuration.
    pub async fn discover(
        &self,
        input: ChannelModelDiscoveryInput,
    ) -> Result<ChannelModelDiscoveryResponse, ChannelModelDiscoveryError> {
        let snapshot = self.runtime.snapshot();
        let channel = compile_channel_discovery_target(&input.into_record(), &snapshot)
            .map_err(|_| ChannelModelDiscoveryError::InvalidConfiguration)?;
        let policy = ResolvedUpstreamPolicy::try_resolve(
            &snapshot.system_settings().upstream_timeouts(),
            channel.upstream_policy(),
        )
        .map_err(|_| ChannelModelDiscoveryError::InvalidConfiguration)?;
        let client = self
            .upstream_clients
            .client_for(channel.upstream_policy(), policy)
            .map_err(|_| ChannelModelDiscoveryError::RequestFailed)?;

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        apply_header_plan(
            &mut headers,
            channel
                .upstream_policy()
                .effective_transforms()
                .request_headers(),
        )
        .map_err(|_| ChannelModelDiscoveryError::InvalidConfiguration)?;
        inject_upstream_auth(&mut headers, &channel)
            .map_err(|_| ChannelModelDiscoveryError::InvalidConfiguration)?;
        strip_explicitly_ignored_client_headers(&mut headers);

        let response = timeout(
            policy.timeouts().response_header(),
            client
                .request(Method::GET, models_url(&channel)?)
                .headers(headers)
                .send(),
        )
        .await
        .map_err(|_| ChannelModelDiscoveryError::ResponseHeaderTimeout)?
        .map_err(|_| ChannelModelDiscoveryError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(ChannelModelDiscoveryError::UpstreamHttpStatus(
                response.status().as_u16(),
            ));
        }

        let body = read_response_body(
            response,
            policy.timeouts().stream_idle(),
            MAX_RESPONSE_BYTES,
        )
        .await?;
        parse_models(&body)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelModelDiscoveryInput {
    pub api_format: String,
    pub base_url: String,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    #[serde(default = "empty_object")]
    pub override_document: Value,
    #[serde(default)]
    pub connect_timeout_ms: Option<i32>,
    #[serde(default)]
    pub response_header_timeout_ms: Option<i32>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    #[serde(default)]
    pub upstream_auth_header_name: Option<String>,
    #[serde(default)]
    pub upstream_api_key: Option<String>,
}

impl ChannelModelDiscoveryInput {
    fn into_record(self) -> ChannelRecord {
        let (upstream_auth_header_name, upstream_api_key) = match self.upstream_auth_kind.as_str() {
            "none" => (None, None),
            "bearer" => (None, self.upstream_api_key),
            _ => (self.upstream_auth_header_name, self.upstream_api_key),
        };
        ChannelRecord {
            id: Uuid::nil(),
            channel_group_id: Uuid::nil(),
            api_format: self.api_format,
            name: "channel-model-discovery".into(),
            base_url: self.base_url,
            enabled: true,
            supports_websocket: false,
            supports_standalone_web_search: false,
            auto_disabled: false,
            auto_disable_allowed: false,
            weight: 1,
            billing_multiplier: Decimal::ONE,
            proxy_id: self.proxy_id,
            config_template_id: self.config_template_id,
            override_document: self.override_document,
            connect_timeout_ms: self.connect_timeout_ms,
            response_header_timeout_ms: self.response_header_timeout_ms,
            stream_idle_timeout_ms: self.stream_idle_timeout_ms,
            upstream_auth_kind: self.upstream_auth_kind,
            upstream_auth_header_name,
            upstream_api_key,
            available_models: Vec::new(),
            test_model: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ChannelModelDiscoveryResponse {
    pub models: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ChannelModelDiscoveryError {
    #[error("channel model discovery configuration is invalid")]
    InvalidConfiguration,
    #[error("upstream model discovery response header timed out")]
    ResponseHeaderTimeout,
    #[error("upstream model discovery request failed")]
    RequestFailed,
    #[error("upstream model discovery returned HTTP {0}")]
    UpstreamHttpStatus(u16),
    #[error("upstream model discovery response body timed out")]
    ResponseBodyTimeout,
    #[error("upstream model discovery response body failed")]
    ResponseBodyFailed,
    #[error("upstream model discovery response is too large")]
    ResponseTooLarge,
    #[error("upstream model discovery response is invalid")]
    InvalidResponse,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

fn models_url(channel: &CompiledChannel) -> Result<reqwest::Url, ChannelModelDiscoveryError> {
    reqwest::Url::parse(&format!(
        "{}/v1/models",
        channel.base_url().as_str().trim_end_matches('/')
    ))
    .map_err(|_| ChannelModelDiscoveryError::InvalidConfiguration)
}

fn inject_upstream_auth(headers: &mut HeaderMap, channel: &CompiledChannel) -> Result<(), ()> {
    match channel.upstream_auth() {
        UpstreamAuth::None => Ok(()),
        UpstreamAuth::Bearer(token) => {
            let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| ())?;
            headers.insert(AUTHORIZATION, value);
            Ok(())
        }
        UpstreamAuth::Header { name, value } => {
            let value = HeaderValue::from_str(value).map_err(|_| ())?;
            headers.insert(name.clone(), value);
            Ok(())
        }
    }
}

async fn read_response_body(
    response: reqwest::Response,
    stream_idle_timeout: Duration,
    max_response_bytes: usize,
) -> Result<Bytes, ChannelModelDiscoveryError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes as u64)
    {
        return Err(ChannelModelDiscoveryError::ResponseTooLarge);
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(max_response_bytes);
    let mut body = BytesMut::with_capacity(capacity);
    let mut stream = response.bytes_stream();
    loop {
        match timeout(stream_idle_timeout, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                if body.len().saturating_add(chunk.len()) > max_response_bytes {
                    return Err(ChannelModelDiscoveryError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Some(Err(_))) => return Err(ChannelModelDiscoveryError::ResponseBodyFailed),
            Ok(None) => return Ok(body.freeze()),
            Err(_) => return Err(ChannelModelDiscoveryError::ResponseBodyTimeout),
        }
    }
}

#[derive(Deserialize)]
struct ModelsEnvelope {
    data: Vec<ModelItem>,
}

#[derive(Deserialize)]
struct ModelItem {
    id: String,
}

fn parse_models(body: &[u8]) -> Result<ChannelModelDiscoveryResponse, ChannelModelDiscoveryError> {
    let envelope: ModelsEnvelope =
        serde_json::from_slice(body).map_err(|_| ChannelModelDiscoveryError::InvalidResponse)?;
    if envelope.data.len() > MAX_DISCOVERED_MODELS {
        return Err(ChannelModelDiscoveryError::InvalidResponse);
    }

    let mut seen = HashSet::with_capacity(envelope.data.len());
    let mut models = Vec::with_capacity(envelope.data.len());
    for item in envelope.data {
        let model = item.id.trim();
        if model.is_empty() || model.len() > MAX_MODEL_ID_BYTES {
            return Err(ChannelModelDiscoveryError::InvalidResponse);
        }
        let model = model.to_owned();
        if seen.insert(model.clone()) {
            models.push(model);
        }
    }
    Ok(ChannelModelDiscoveryResponse { models })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::{ChannelModelDiscoveryInput, ChannelModelDiscoveryService};
    use crate::{
        domain::CompiledRuntimeConfig, runtime_config::RuntimeConfig,
        upstream::UpstreamClientRegistry,
    };

    async fn models(headers: HeaderMap) -> Result<Json<serde_json::Value>, StatusCode> {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some("Bearer discovery-secret")
            || headers
                .get("x-discovery-source")
                .and_then(|value| value.to_str().ok())
                != Some("channel-form")
            || ["forwarded", "x-forwarded-for", "cf-connecting-ip"]
                .iter()
                .any(|name| headers.contains_key(*name))
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(Json(json!({
            "object": "list",
            "data": [
                {"id": "model-b"},
                {"id": "model-a"},
                {"id": "model-b"}
            ]
        })))
    }

    #[tokio::test]
    async fn discovers_unique_models_with_draft_headers_and_authentication() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/models", get(models)))
                .await
                .unwrap();
        });
        let service = ChannelModelDiscoveryService::new(
            Arc::new(RuntimeConfig::new(CompiledRuntimeConfig::empty())),
            Arc::new(UpstreamClientRegistry::new()),
        );

        let response = service
            .discover(ChannelModelDiscoveryInput {
                api_format: "open_ai_chat_completions".into(),
                base_url: format!("http://{address}"),
                proxy_id: None,
                config_template_id: None,
                override_document: json!({
                    "version": 1,
                    "api_format": "open_ai_chat_completions",
                    "request_headers": {
                        "set": {
                            "cf-connecting-ip": "192.0.2.1",
                            "forwarded": "for=192.0.2.1;proto=https",
                            "x-discovery-source": "channel-form",
                            "x-forwarded-for": "192.0.2.1"
                        }
                    }
                }),
                connect_timeout_ms: None,
                response_header_timeout_ms: None,
                stream_idle_timeout_ms: None,
                upstream_auth_kind: "bearer".into(),
                upstream_auth_header_name: None,
                upstream_api_key: Some("discovery-secret".into()),
            })
            .await
            .unwrap();

        assert_eq!(response.models, vec!["model-b", "model-a"]);
        task.abort();
    }
}
