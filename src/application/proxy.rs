use std::{
    collections::{BTreeSet, HashSet},
    error::Error,
    io,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::{Body, Bytes, to_bytes},
    http::{
        HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri,
        header::{
            ACCEPT_ENCODING, AUTHORIZATION, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, HOST,
            PROXY_AUTHORIZATION,
        },
    },
    response::{IntoResponse, Response as AxumResponse},
};
use futures_util::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    admission::{AdmissionError, AdmissionLease, AdmissionRuntime},
    application::{NoopRequestLogSink, RequestLogSink},
    domain::{
        ApiFormat, ApiKeyPermission, CompiledApiKey, CompiledChannel, CompiledModelRule,
        RequestLogEvent, RequestLogOutcome, UpstreamAuth,
    },
    routing::{ChannelLease, RoutingRuntime, SelectionResult},
    runtime_config::{RuntimeConfig, UpstreamConfig},
    transforms::{
        SseEventPatchPlan, SseTransformer, apply_header_plan, apply_json_patch_plan,
        apply_response_header_plan, parse_connection_header_names,
    },
    upstream::{ResolvedUpstreamPolicy, UpstreamClientRegistry},
};

/// Data-plane use case backed by a single immutable configuration snapshot per
/// request and a process-shared upstream client registry.
#[derive(Clone)]
pub struct ProxyService {
    runtime: Arc<RuntimeConfig>,
    upstream_clients: Arc<UpstreamClientRegistry>,
    upstream_defaults: UpstreamConfig,
    max_request_body_bytes: usize,
    request_log_sink: Arc<dyn RequestLogSink>,
    routing: RoutingRuntime,
    admission: AdmissionRuntime,
}

impl ProxyService {
    pub fn new(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream: &UpstreamConfig,
    ) -> Result<Self, reqwest::Error> {
        Self::with_log_sink(
            runtime,
            max_request_body_bytes,
            upstream,
            Arc::new(NoopRequestLogSink),
        )
    }

    pub fn with_log_sink(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream: &UpstreamConfig,
        request_log_sink: Arc<dyn RequestLogSink>,
    ) -> Result<Self, reqwest::Error> {
        Self::with_log_sink_and_routing(
            runtime,
            max_request_body_bytes,
            upstream,
            request_log_sink,
            RoutingRuntime::new(crate::routing::PassiveHealthPolicy::default()),
        )
    }

    pub fn with_log_sink_and_routing(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream: &UpstreamConfig,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
    ) -> Result<Self, reqwest::Error> {
        Self::with_dependencies(
            runtime,
            max_request_body_bytes,
            upstream,
            request_log_sink,
            routing,
            AdmissionRuntime::new(),
        )
    }

    pub fn with_dependencies(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream: &UpstreamConfig,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
    ) -> Result<Self, reqwest::Error> {
        Self::with_dependencies_and_registry(
            runtime,
            max_request_body_bytes,
            upstream,
            Arc::new(UpstreamClientRegistry::new()),
            request_log_sink,
            routing,
            admission,
        )
    }

    /// Constructs a proxy with a process-shared registry supplied by the host.
    pub fn with_dependencies_and_registry(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream: &UpstreamConfig,
        upstream_clients: Arc<UpstreamClientRegistry>,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            runtime,
            upstream_clients,
            upstream_defaults: upstream.clone(),
            max_request_body_bytes,
            request_log_sink,
            routing,
            admission,
        })
    }

    pub async fn proxy(
        &self,
        api_format: ApiFormat,
        request: Request<Body>,
    ) -> Result<AxumResponse, ProxyError> {
        let started_at = Instant::now();
        let started_wall_at = chrono::Utc::now();
        let (parts, body) = request.into_parts();
        let client_key = match parse_bearer_token(&parts.headers) {
            Ok(value) => value,
            Err(error) => {
                trace_unlogged("invalid_api_key");
                return Err(error);
            }
        };
        let snapshot = self.runtime.snapshot();
        let api_key = match snapshot.authenticate(client_key) {
            Some(value) => value,
            None => {
                trace_unlogged("invalid_or_expired_api_key");
                return Err(ProxyError::invalid_api_key());
            }
        };
        if !api_key.permits(api_format, ApiKeyPermission::Proxy) {
            trace_unlogged("proxy_permission_denied");
            return Err(ProxyError::forbidden(
                "This API key cannot proxy requests in this API format.",
            ));
        }
        let admission = match self.admission.admit(&api_key) {
            Ok(lease) => lease,
            Err(AdmissionError::RateLimited { retry_after }) => {
                trace_unlogged("rate_limited");
                return Err(ProxyError::rate_limited(retry_after));
            }
            Err(AdmissionError::ConcurrentLimited) => {
                trace_unlogged("concurrent_limited");
                return Err(ProxyError::concurrent_limited());
            }
            Err(AdmissionError::InsufficientQuota) => {
                trace_unlogged("insufficient_quota");
                return Err(ProxyError::insufficient_quota());
            }
        };

        let original_body = match to_bytes(body, self.max_request_body_bytes).await {
            Ok(value) => value,
            Err(error) => {
                trace_unlogged("unreadable_or_oversized_body");
                return Err(request_body_error(error));
            }
        };
        let parsed = match parse_request(&original_body) {
            Ok(value) => value,
            Err(error) => {
                trace_unlogged("malformed_or_overlength_model");
                return Err(error);
            }
        };
        let route = match self
            .routing
            .select(&snapshot, &api_key, api_format, &parsed.model)
        {
            SelectionResult::Selected(route) => route,
            SelectionResult::UnknownOrInaccessibleModel => {
                self.record_rejected(
                    &api_key,
                    api_format,
                    &parsed.model,
                    parsed.streamed,
                    started_wall_at,
                    started_at,
                );
                return Err(ProxyError::unknown_model(&parsed.model));
            }
            SelectionResult::NoHealthyChannel { rule } => {
                self.record_no_healthy_channel(
                    &api_key,
                    api_format,
                    &parsed.model,
                    parsed.streamed,
                    &rule,
                    started_wall_at,
                    started_at,
                );
                return Err(ProxyError::no_healthy_channel());
            }
        };
        let mut completion = CompletionGuard::new(
            Arc::clone(&self.request_log_sink),
            &api_key,
            &parsed.model,
            parsed.streamed,
            api_format,
            &route.rule,
            &route.channel,
            route.lease,
            admission,
            started_wall_at,
            started_at,
        );
        let transforms = route.channel.upstream_policy().effective_transforms();
        let body = match rewrite_model_alias(original_body, &parsed.model, &route.rule) {
            Ok(value) => value,
            Err(error) => {
                completion.finish(RequestOutcome::ClientRequestError);
                return Err(error);
            }
        };
        let body = match apply_json_patch_plan(body, transforms.request_json()) {
            Ok(value) => value,
            Err(_) => {
                completion.finish(RequestOutcome::ClientRequestError);
                return Err(ProxyError::transform_failed());
            }
        };

        // Apply the plan before hop-by-hop cleanup so `HeaderPlan` can reject
        // dynamically protected names declared by the client `Connection`
        // header. Cleanup then removes those client-controlled names again.
        let mut headers = parts.headers.clone();
        if apply_header_plan(&mut headers, transforms.request_headers()).is_err() {
            completion.finish(RequestOutcome::ClientRequestError);
            return Err(ProxyError::transform_failed());
        }
        let mut headers = forward_request_headers(&headers);
        let sse_transform_active = transforms.sse_event_patches().has_operations();
        if sse_transform_active {
            headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        }

        let url = match upstream_url(&route.channel, &parts.uri) {
            Ok(value) => value,
            Err(error) => {
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return Err(error);
            }
        };
        if let Err(error) = inject_upstream_auth(&mut headers, &route.channel) {
            completion.finish(RequestOutcome::UpstreamUnavailable);
            return Err(error);
        }
        let upstream_policy = match ResolvedUpstreamPolicy::try_resolve(
            &self.upstream_defaults,
            route.channel.upstream_policy(),
        ) {
            Ok(policy) => policy,
            Err(_) => {
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return Err(ProxyError::upstream_unavailable());
            }
        };
        let upstream_client = match self
            .upstream_clients
            .client_for(route.channel.upstream_policy(), upstream_policy)
        {
            Ok(client) => client,
            Err(_) => {
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return Err(ProxyError::upstream_unavailable());
            }
        };

        let upstream_request = upstream_client
            .request(parts.method, url)
            .headers(headers)
            .body(body);
        let upstream_response = match timeout(
            upstream_policy.timeouts().response_header(),
            upstream_request.send(),
        )
        .await
        {
            Err(_) => {
                completion.probe_failed();
                completion.finish(RequestOutcome::ResponseHeaderTimeout);
                return Err(ProxyError::response_header_timeout());
            }
            Ok(Err(error)) => {
                if error.is_timeout() && error.is_connect() {
                    completion.connection_failed();
                    completion.finish(RequestOutcome::ConnectTimeout);
                    return Err(ProxyError::connect_timeout());
                }
                if error.is_connect() {
                    completion.connection_failed();
                }
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return Err(ProxyError::upstream_unavailable());
            }
            Ok(Ok(response)) => {
                completion.response_headers_received();
                response
            }
        };

        response_from_upstream(
            upstream_response,
            upstream_policy.timeouts().stream_idle(),
            completion,
            transforms.response_headers(),
            transforms.sse_event_patches().clone(),
        )
    }

    fn record_rejected(
        &self,
        api_key: &CompiledApiKey,
        api_format: ApiFormat,
        client_model: &str,
        streamed: bool,
        started_at: chrono::DateTime<chrono::Utc>,
        started: Instant,
    ) {
        let elapsed = clamp_duration_ms(started.elapsed());
        let event = RequestLogEvent {
            id: Uuid::new_v4(),
            started_at,
            completed_at: completed_at(started_at, started.elapsed()),
            user_id: api_key.user_id(),
            api_key_id: api_key.id(),
            api_format,
            client_model: client_model.to_owned(),
            upstream_model: None,
            model_rule_id: None,
            channel_group_id: None,
            channel_id: None,
            model_id: None,
            outcome: RequestLogOutcome::Rejected,
            response_status_code: Some(StatusCode::NOT_FOUND.as_u16()),
            streamed,
            ttft_ms: None,
            total_duration_ms: elapsed,
            error_code: Some("model_not_found"),
        };
        tracing::info!(event = "proxy_request_completed", api_key_id = %api_key.id(), api_format = ?api_format, outcome = "rejected", "proxy request completed");
        self.request_log_sink.try_record(event);
    }

    #[allow(clippy::too_many_arguments)]
    fn record_no_healthy_channel(
        &self,
        api_key: &CompiledApiKey,
        api_format: ApiFormat,
        client_model: &str,
        streamed: bool,
        rule: &CompiledModelRule,
        started_at: chrono::DateTime<chrono::Utc>,
        started: Instant,
    ) {
        let event = RequestLogEvent {
            id: Uuid::new_v4(),
            started_at,
            completed_at: completed_at(started_at, started.elapsed()),
            user_id: api_key.user_id(),
            api_key_id: api_key.id(),
            api_format,
            client_model: client_model.to_owned(),
            upstream_model: Some(rule.upstream_model().to_owned()),
            model_rule_id: Some(rule.id()),
            channel_group_id: None,
            channel_id: None,
            model_id: Some(rule.model_id()),
            outcome: RequestLogOutcome::Failed,
            response_status_code: Some(StatusCode::SERVICE_UNAVAILABLE.as_u16()),
            streamed,
            ttft_ms: None,
            total_duration_ms: clamp_duration_ms(started.elapsed()),
            error_code: Some("no_healthy_channel"),
        };
        tracing::info!(event = "proxy_request_completed", api_key_id = %api_key.id(), api_format = ?api_format, outcome = "no_healthy_channel", "proxy request completed");
        self.request_log_sink.try_record(event);
    }

    pub fn list_models(&self, headers: &HeaderMap) -> Result<ModelsResponse, ProxyError> {
        let client_key = parse_bearer_token(headers)?;
        let snapshot = self.runtime.snapshot();
        let api_key = snapshot
            .authenticate(client_key)
            .ok_or_else(ProxyError::invalid_api_key)?;

        let can_list_models = [ApiFormat::OpenAiChatCompletions, ApiFormat::OpenAiResponses]
            .into_iter()
            .any(|api_format| {
                api_key.permits(api_format, ApiKeyPermission::Proxy)
                    && api_key.permits(api_format, ApiKeyPermission::ModelsRead)
            });
        if !can_list_models {
            return Err(ProxyError::forbidden(
                "This API key cannot list models in any API format.",
            ));
        }

        let models = [ApiFormat::OpenAiChatCompletions, ApiFormat::OpenAiResponses]
            .into_iter()
            .flat_map(|api_format| snapshot.models_for(&api_key, api_format))
            .collect::<BTreeSet<_>>();

        Ok(ModelsResponse {
            object: "list",
            data: models
                .into_iter()
                .map(|id| ModelResponse {
                    id: id.to_string(),
                    object: "model",
                    owned_by: "ai-gateway",
                })
                .collect(),
        })
    }
}

#[derive(Serialize)]
pub struct ModelsResponse {
    object: &'static str,
    data: Vec<ModelResponse>,
}

#[derive(Serialize)]
struct ModelResponse {
    id: String,
    object: &'static str,
    owned_by: &'static str,
}

#[derive(Debug)]
pub struct ProxyError {
    status: StatusCode,
    message: String,
    error_type: &'static str,
    param: Option<&'static str>,
    code: Option<&'static str>,
    authenticate: bool,
    retry_after: Option<u64>,
}

impl ProxyError {
    fn invalid_api_key() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "Invalid or missing API key.".to_owned(),
            error_type: "authentication_error",
            param: None,
            code: "invalid_api_key".into(),
            authenticate: true,
            retry_after: None,
        }
    }

    fn forbidden(message: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.to_owned(),
            error_type: "permission_error",
            param: None,
            code: "permission_denied".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn payload_too_large() -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "Request body exceeds the configured size limit.".to_owned(),
            error_type: "invalid_request_error",
            param: None,
            code: "request_too_large".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn invalid_request(message: &'static str, param: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.to_owned(),
            error_type: "invalid_request_error",
            param: Some(param),
            code: "invalid_request".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn transform_failed() -> Self {
        Self::invalid_request("Request transform could not be applied.", "body")
    }

    fn response_transform_failed() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "Upstream response transform could not be applied.".to_owned(),
            error_type: "api_error",
            param: None,
            code: Some("response_transform_failed"),
            authenticate: false,
            retry_after: None,
        }
    }

    fn unknown_model(model: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: format!("The model `{model}` does not exist or is unavailable."),
            error_type: "invalid_request_error",
            param: Some("model"),
            code: "model_not_found".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn upstream_unavailable() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "The selected upstream channel could not be reached.".to_owned(),
            error_type: "api_error",
            param: None,
            code: "upstream_unavailable".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn response_header_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "The selected upstream channel did not return response headers in time."
                .to_owned(),
            error_type: "api_error",
            param: None,
            code: "response_header_timeout".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn connect_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "The selected upstream channel could not be connected in time.".to_owned(),
            error_type: "api_error",
            param: None,
            code: "connect_timeout".into(),
            authenticate: false,
            retry_after: None,
        }
    }

    fn no_healthy_channel() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "No healthy upstream channel is currently available for this model."
                .to_owned(),
            error_type: "api_error",
            param: None,
            code: Some("no_healthy_channel"),
            authenticate: false,
            retry_after: None,
        }
    }

    fn rate_limited(retry_after: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "Request rate limit exceeded.".to_owned(),
            error_type: "rate_limit_error",
            param: None,
            code: Some("rate_limit_exceeded"),
            authenticate: false,
            retry_after: Some(retry_after),
        }
    }
    fn concurrent_limited() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "Concurrent request limit exceeded.".to_owned(),
            error_type: "rate_limit_error",
            param: None,
            code: Some("concurrent_limit_exceeded"),
            authenticate: false,
            retry_after: None,
        }
    }
    fn insufficient_quota() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "Quota has been exhausted.".to_owned(),
            error_type: "insufficient_quota",
            param: None,
            code: Some("insufficient_quota"),
            authenticate: false,
            retry_after: None,
        }
    }
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> AxumResponse {
        let mut response = (
            self.status,
            Json(json!({
                "error": {
                    "message": self.message,
                    "type": self.error_type,
                    "param": self.param,
                    "code": self.code,
                }
            })),
        )
            .into_response();
        if self.authenticate {
            response
                .headers_mut()
                .insert("www-authenticate", HeaderValue::from_static("Bearer"));
        }
        if let Some(retry_after) = self.retry_after {
            if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
                response.headers_mut().insert("retry-after", value);
            }
        }
        response
    }
}

#[derive(Deserialize)]
struct RequestProbe {
    model: String,
    #[serde(default)]
    stream: bool,
}

struct ParsedRequest {
    model: String,
    streamed: bool,
}

fn parse_request(body: &[u8]) -> Result<ParsedRequest, ProxyError> {
    let probe = serde_json::from_slice::<RequestProbe>(body).map_err(|_| {
        ProxyError::invalid_request("Request body must contain a string model.", "model")
    })?;
    if probe.model.trim().is_empty() {
        return Err(ProxyError::invalid_request(
            "Request body must contain a non-empty model.",
            "model",
        ));
    }
    if probe.model.chars().count() > 300 {
        return Err(ProxyError::invalid_request(
            "Request model exceeds the supported length.",
            "model",
        ));
    }
    Ok(ParsedRequest {
        model: probe.model,
        streamed: probe.stream,
    })
}

/// Documents the deliberate no-row policy for requests that cannot be safely
/// represented by the append-only request_logs schema.
fn trace_unlogged(reason: &'static str) {
    tracing::debug!(
        event = "proxy_request_unlogged",
        reason,
        "proxy request has no request log"
    );
}

fn request_body_error(error: axum::Error) -> ProxyError {
    if error.into_inner().is::<http_body_util::LengthLimitError>() {
        ProxyError::payload_too_large()
    } else {
        ProxyError::invalid_request("Request body could not be read.", "body")
    }
}

fn rewrite_model_alias(
    original_body: Bytes,
    client_model: &str,
    rule: &CompiledModelRule,
) -> Result<Bytes, ProxyError> {
    if client_model == rule.upstream_model() {
        return Ok(original_body);
    }

    let mut value = serde_json::from_slice::<Value>(&original_body).map_err(|_| {
        ProxyError::invalid_request("Request body must contain a JSON object.", "model")
    })?;
    let object = value.as_object_mut().ok_or_else(|| {
        ProxyError::invalid_request("Request body must contain a JSON object.", "model")
    })?;
    object.insert(
        "model".to_owned(),
        Value::String(rule.upstream_model().to_owned()),
    );
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| ProxyError::invalid_request("Request body cannot be rewritten.", "model"))
}

fn parse_bearer_token(headers: &HeaderMap) -> Result<&str, ProxyError> {
    let value = headers
        .get(AUTHORIZATION)
        .ok_or_else(ProxyError::invalid_api_key)?
        .to_str()
        .map_err(|_| ProxyError::invalid_api_key())?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or_else(ProxyError::invalid_api_key)?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.chars().any(char::is_whitespace)
    {
        return Err(ProxyError::invalid_api_key());
    }
    Ok(token)
}

fn upstream_url(channel: &CompiledChannel, uri: &Uri) -> Result<reqwest::Url, ProxyError> {
    let base = channel.base_url().as_str().trim_end_matches('/');
    let query = uri
        .query()
        .map_or_else(String::new, |query| format!("?{query}"));
    let target = format!("{base}{}{query}", uri.path());
    reqwest::Url::parse(&target).map_err(|_| ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: "The selected upstream channel has an invalid target URL.".to_owned(),
        error_type: "api_error",
        param: None,
        code: "invalid_upstream_url".into(),
        authenticate: false,
        retry_after: None,
    })
}

fn inject_upstream_auth(
    headers: &mut HeaderMap,
    channel: &CompiledChannel,
) -> Result<(), ProxyError> {
    let invalid = || ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: "The selected upstream channel has invalid credentials.".to_owned(),
        error_type: "api_error",
        param: None,
        code: "invalid_upstream_credentials".into(),
        authenticate: false,
        retry_after: None,
    };
    match channel.upstream_auth() {
        UpstreamAuth::None => {}
        UpstreamAuth::Bearer(token) => {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| invalid())?,
            );
        }
        UpstreamAuth::Header { name, value } => {
            headers.insert(
                name.clone(),
                HeaderValue::from_str(value).map_err(|_| invalid())?,
            );
        }
    }
    Ok(())
}

fn forward_request_headers(headers: &HeaderMap) -> HeaderMap {
    forward_headers(headers, true)
}

fn forward_response_headers(headers: &HeaderMap) -> HeaderMap {
    forward_headers(headers, false)
}

fn forward_headers(headers: &HeaderMap, request: bool) -> HeaderMap {
    let connection_names = connection_header_names(headers);
    let mut forwarded = HeaderMap::new();
    for (name, value) in headers {
        if is_hop_by_hop(name, &connection_names)
            || (request
                && matches!(
                    *name,
                    HOST | CONTENT_LENGTH | AUTHORIZATION | PROXY_AUTHORIZATION
                ))
        {
            continue;
        }
        forwarded.append(name.clone(), value.clone());
    }
    forwarded
}

fn connection_header_names(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all(CONNECTION)
        .iter()
        .flat_map(parse_connection_header_names)
        .collect()
}

fn is_hop_by_hop(name: &HeaderName, connection_names: &HashSet<HeaderName>) -> bool {
    connection_names.contains(name)
        || matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        )
}

fn response_from_upstream(
    upstream_response: reqwest::Response,
    stream_idle_timeout: Duration,
    mut completion: CompletionGuard,
    response_headers: &crate::transforms::HeaderPlan,
    sse_event_patches: SseEventPatchPlan,
) -> Result<AxumResponse, ProxyError> {
    let upstream_status = upstream_response.status();
    let status = StatusCode::from_u16(upstream_status.as_u16())
        .expect("reqwest status code is valid for an Axum response");
    completion.set_upstream_status(upstream_status.as_u16());
    // Framing is a property of the upstream response, not of configured
    // presentation headers. Response plans are also forbidden from touching
    // these headers, but classify first to keep that invariant explicit.
    let original_upstream_headers = upstream_response.headers();
    let transform_sse =
        sse_event_patches.has_operations() && is_sse_response(original_upstream_headers);
    let sse_has_identity_encoding = has_identity_content_encoding(original_upstream_headers);
    let mut upstream_headers = original_upstream_headers.clone();
    if apply_response_header_plan(&mut upstream_headers, response_headers).is_err() {
        completion.set_client_visible_status(StatusCode::BAD_GATEWAY.as_u16());
        completion.finish(RequestOutcome::ResponseTransformFailed);
        return Err(ProxyError::response_transform_failed());
    }
    if transform_sse && !sse_has_identity_encoding {
        completion.set_client_visible_status(StatusCode::BAD_GATEWAY.as_u16());
        completion.finish(RequestOutcome::ResponseTransformFailed);
        return Err(ProxyError::response_transform_failed());
    }
    let mut headers = forward_response_headers(&upstream_headers);
    if transform_sse {
        remove_transformed_entity_headers(&mut headers);
    }
    let expected_body_bytes = (!transform_sse)
        .then(|| upstream_response.content_length())
        .flatten();
    if response_has_no_body(status) || expected_body_bytes == Some(0) {
        completion.finish(if upstream_status.is_success() {
            RequestOutcome::Succeeded
        } else {
            RequestOutcome::UpstreamHttpError
        });
        let mut response = Response::new(Body::empty());
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        return Ok(response);
    }
    let stream = timed_upstream_stream(
        upstream_response.bytes_stream(),
        stream_idle_timeout,
        completion,
        upstream_status.is_success(),
        expected_body_bytes,
        transform_sse.then(|| SseTransformer::new(sse_event_patches)),
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

fn is_sse_response(headers: &HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
}

fn has_identity_content_encoding(headers: &HeaderMap) -> bool {
    headers.get_all(CONTENT_ENCODING).iter().all(|value| {
        value
            .to_str()
            .is_ok_and(|value| value.trim().eq_ignore_ascii_case("identity"))
    })
}

fn remove_transformed_entity_headers(headers: &mut HeaderMap) {
    for name in [
        CONTENT_LENGTH,
        HeaderName::from_static("etag"),
        HeaderName::from_static("last-modified"),
        HeaderName::from_static("content-md5"),
        HeaderName::from_static("digest"),
        HeaderName::from_static("content-digest"),
        HeaderName::from_static("repr-digest"),
    ] {
        headers.remove(name);
    }
}

fn response_has_no_body(status: StatusCode) -> bool {
    status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
}

type UpstreamByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
type BodyStreamError = Box<dyn Error + Send + Sync>;

struct StreamState {
    upstream: Option<UpstreamByteStream>,
    idle_timeout: Duration,
    completion: CompletionGuard,
    upstream_succeeded: bool,
    remaining_bytes: Option<u64>,
    sse_transformer: Option<SseTransformer>,
    yield_after_sse_frame: bool,
}

fn timed_upstream_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    idle_timeout: Duration,
    completion: CompletionGuard,
    upstream_succeeded: bool,
    remaining_bytes: Option<u64>,
    sse_transformer: Option<SseTransformer>,
) -> impl Stream<Item = Result<Bytes, BodyStreamError>> + Send {
    stream::unfold(
        StreamState {
            upstream: Some(Box::pin(upstream)),
            idle_timeout,
            completion,
            upstream_succeeded,
            remaining_bytes,
            sse_transformer,
            yield_after_sse_frame: false,
        },
        |mut state| async move {
            if state.yield_after_sse_frame {
                state.yield_after_sse_frame = false;
                tokio::task::yield_now().await;
            }
            loop {
                if let Some(transformer) = &mut state.sse_transformer {
                    match transformer.next_frame() {
                        Ok(Some(frame)) => {
                            record_stream_bytes(&mut state, &frame);
                            state.yield_after_sse_frame = true;
                            return Some((Ok(frame), state));
                        }
                        Ok(None) => {}
                        Err(_) => {
                            state.upstream.take();
                            state.sse_transformer = None;
                            state
                                .completion
                                .finish(RequestOutcome::ResponseTransformFailed);
                            let error: BodyStreamError = Box::new(ResponseTransformBodyError);
                            return Some((Err(error), state));
                        }
                    }
                }
                let next = match state.upstream.as_mut() {
                    Some(upstream) => timeout(state.idle_timeout, upstream.next()).await,
                    None => return None,
                };

                match next {
                    Ok(Some(Ok(bytes))) => {
                        if let Some(transformer) = &mut state.sse_transformer {
                            transformer.push(bytes);
                            continue;
                        }
                        record_stream_bytes(&mut state, &bytes);
                        return Some((Ok(bytes), state));
                    }
                    Ok(Some(Err(error))) => {
                        state.upstream.take();
                        state.completion.finish(RequestOutcome::UpstreamBodyError);
                        let error: BodyStreamError = Box::new(error);
                        return Some((Err(error), state));
                    }
                    Ok(None) => {
                        state.upstream.take();
                        if let Some(transformer) = &mut state.sse_transformer {
                            if let Some(residual) = transformer.finish() {
                                record_stream_bytes(&mut state, &residual);
                                state.completion.finish(if state.upstream_succeeded {
                                    RequestOutcome::Succeeded
                                } else {
                                    RequestOutcome::UpstreamHttpError
                                });
                                return Some((Ok(residual), state));
                            }
                        }
                        state.completion.finish(if state.upstream_succeeded {
                            RequestOutcome::Succeeded
                        } else {
                            RequestOutcome::UpstreamHttpError
                        });
                        return None;
                    }
                    Err(_) => {
                        state.upstream.take();
                        state.completion.finish(RequestOutcome::StreamIdleTimeout);
                        let error: BodyStreamError = Box::new(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "upstream response stream was idle for too long",
                        ));
                        return Some((Err(error), state));
                    }
                }
            }
        },
    )
}

fn record_stream_bytes(state: &mut StreamState, bytes: &Bytes) {
    if !bytes.is_empty() {
        state.completion.record_first_byte();
    }
    if let Some(remaining) = &mut state.remaining_bytes {
        *remaining = remaining.saturating_sub(bytes.len() as u64);
        if *remaining == 0 {
            state.completion.finish(if state.upstream_succeeded {
                RequestOutcome::Succeeded
            } else {
                RequestOutcome::UpstreamHttpError
            });
        }
    }
}

#[derive(Debug)]
struct ResponseTransformBodyError;

impl std::fmt::Display for ResponseTransformBodyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("response_transform_failed")
    }
}

impl Error for ResponseTransformBodyError {}

#[derive(Clone, Copy)]
enum RequestOutcome {
    Succeeded,
    UpstreamHttpError,
    ConnectTimeout,
    ResponseHeaderTimeout,
    UpstreamUnavailable,
    UpstreamBodyError,
    StreamIdleTimeout,
    ResponseTransformFailed,
    Cancelled,
    ClientRequestError,
}

impl RequestOutcome {
    const fn tracing_outcome(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::UpstreamHttpError => "upstream_http_error",
            Self::ConnectTimeout => "connect_timeout",
            Self::ResponseHeaderTimeout => "response_header_timeout",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::UpstreamBodyError => "upstream_body_error",
            Self::StreamIdleTimeout => "stream_idle_timeout",
            Self::ResponseTransformFailed => "response_transform_failed",
            Self::Cancelled => "cancelled",
            Self::ClientRequestError => "client_request_error",
        }
    }

    const fn log_outcome(self) -> RequestLogOutcome {
        match self {
            Self::Succeeded => RequestLogOutcome::Succeeded,
            Self::Cancelled => RequestLogOutcome::Cancelled,
            Self::UpstreamHttpError
            | Self::ConnectTimeout
            | Self::ResponseHeaderTimeout
            | Self::UpstreamUnavailable
            | Self::UpstreamBodyError
            | Self::StreamIdleTimeout
            | Self::ResponseTransformFailed
            | Self::ClientRequestError => RequestLogOutcome::Failed,
        }
    }

    const fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Succeeded => None,
            Self::UpstreamHttpError => Some("upstream_http_error"),
            Self::ConnectTimeout => Some("connect_timeout"),
            Self::ResponseHeaderTimeout => Some("response_header_timeout"),
            Self::UpstreamUnavailable => Some("upstream_unavailable"),
            Self::UpstreamBodyError => Some("upstream_body_error"),
            Self::StreamIdleTimeout => Some("stream_idle_timeout"),
            Self::ResponseTransformFailed => Some("response_transform_failed"),
            Self::Cancelled => Some("client_cancelled"),
            Self::ClientRequestError => Some("invalid_request"),
        }
    }

    const fn fallback_status(self) -> Option<u16> {
        match self {
            Self::ConnectTimeout | Self::ResponseHeaderTimeout => {
                Some(StatusCode::GATEWAY_TIMEOUT.as_u16())
            }
            Self::UpstreamUnavailable => Some(StatusCode::BAD_GATEWAY.as_u16()),
            Self::ResponseTransformFailed => Some(StatusCode::BAD_GATEWAY.as_u16()),
            Self::ClientRequestError => Some(StatusCode::BAD_REQUEST.as_u16()),
            Self::Succeeded
            | Self::UpstreamHttpError
            | Self::UpstreamBodyError
            | Self::StreamIdleTimeout
            | Self::Cancelled => None,
        }
    }
}

struct CompletionContext {
    event_id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    client_model: String,
    upstream_model: String,
    model_rule_id: Uuid,
    channel_group_id: Uuid,
    channel_id: Uuid,
    model_id: Uuid,
    api_format: ApiFormat,
    streamed: bool,
    started_wall_at: chrono::DateTime<chrono::Utc>,
    started_at: Instant,
    first_byte_at: Option<Duration>,
    upstream_status: Option<u16>,
    client_visible_status: Option<u16>,
    sink: Arc<dyn RequestLogSink>,
}

/// Emits exactly one event, including when Axum drops an in-flight response
/// body after a downstream client disconnects.
struct CompletionGuard {
    context: Option<CompletionContext>,
    lease: Option<ChannelLease>,
    _admission: Option<AdmissionLease>,
}

impl CompletionGuard {
    #[allow(clippy::too_many_arguments)] // terminal event requires all selected-route context
    fn new(
        sink: Arc<dyn RequestLogSink>,
        api_key: &CompiledApiKey,
        client_model: &str,
        streamed: bool,
        api_format: ApiFormat,
        rule: &CompiledModelRule,
        channel: &CompiledChannel,
        lease: ChannelLease,
        admission: AdmissionLease,
        started_wall_at: chrono::DateTime<chrono::Utc>,
        started_at: Instant,
    ) -> Self {
        Self {
            context: Some(CompletionContext {
                event_id: Uuid::new_v4(),
                user_id: api_key.user_id(),
                api_key_id: api_key.id(),
                client_model: client_model.to_owned(),
                upstream_model: rule.upstream_model().to_owned(),
                model_rule_id: rule.id(),
                channel_group_id: channel.group_id(),
                channel_id: channel.id(),
                model_id: rule.model_id(),
                api_format,
                streamed,
                started_wall_at,
                started_at,
                first_byte_at: None,
                upstream_status: None,
                client_visible_status: None,
                sink,
            }),
            lease: Some(lease),
            _admission: Some(admission),
        }
    }

    fn set_upstream_status(&mut self, status: u16) {
        if let Some(context) = &mut self.context {
            context.upstream_status = Some(status);
        }
    }

    fn set_client_visible_status(&mut self, status: u16) {
        if let Some(context) = &mut self.context {
            context.client_visible_status = Some(status);
        }
    }

    fn record_first_byte(&mut self) {
        if let Some(context) = &mut self.context {
            context
                .first_byte_at
                .get_or_insert_with(|| context.started_at.elapsed());
        }
    }

    fn response_headers_received(&mut self) {
        if let Some(lease) = &mut self.lease {
            lease.response_headers_received();
        }
    }

    fn connection_failed(&mut self) {
        if let Some(lease) = &mut self.lease {
            lease.connection_failed();
        }
    }

    fn probe_failed(&mut self) {
        if let Some(lease) = &mut self.lease {
            lease.probe_failed();
        }
    }

    fn finish(&mut self, outcome: RequestOutcome) {
        let Some(context) = self.context.take() else {
            return;
        };
        tracing::info!(
            event = "proxy_request_completed",
            api_key_id = %context.api_key_id,
            client_model = %context.client_model,
            upstream_model = %context.upstream_model,
            channel_id = %context.channel_id,
            api_format = ?context.api_format,
            upstream_status = ?context.upstream_status,
            latency_ms = context.started_at.elapsed().as_millis(),
            ttft_ms = ?context.first_byte_at.map(|duration| duration.as_millis()),
            outcome = outcome.tracing_outcome(),
            "proxy request completed"
        );
        let total_duration_ms = clamp_duration_ms(context.started_at.elapsed());
        let event = RequestLogEvent {
            id: context.event_id,
            started_at: context.started_wall_at,
            completed_at: completed_at(context.started_wall_at, context.started_at.elapsed()),
            user_id: context.user_id,
            api_key_id: context.api_key_id,
            api_format: context.api_format,
            client_model: context.client_model,
            upstream_model: Some(context.upstream_model),
            model_rule_id: Some(context.model_rule_id),
            channel_group_id: Some(context.channel_group_id),
            channel_id: Some(context.channel_id),
            model_id: Some(context.model_id),
            outcome: outcome.log_outcome(),
            response_status_code: context
                .client_visible_status
                .or(context.upstream_status)
                .or(outcome.fallback_status()),
            streamed: context.streamed,
            ttft_ms: context.first_byte_at.map(clamp_duration_ms),
            total_duration_ms,
            error_code: outcome.error_code(),
        };
        context.sink.try_record(event);
    }
}

fn clamp_duration_ms(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

fn completed_at(
    started_at: chrono::DateTime<chrono::Utc>,
    elapsed: Duration,
) -> chrono::DateTime<chrono::Utc> {
    let elapsed = chrono::Duration::from_std(elapsed)
        .unwrap_or_else(|_| chrono::Duration::milliseconds(i64::MAX));
    started_at.checked_add_signed(elapsed).unwrap_or(started_at)
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.finish(RequestOutcome::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header::CONNECTION};
    use std::{collections::HashSet, sync::Arc};

    use reqwest::{Url, header::HeaderName};
    use uuid::Uuid;

    use super::{
        forward_request_headers, forward_response_headers, inject_upstream_auth,
        parse_bearer_token, response_has_no_body,
    };
    use crate::domain::{ApiFormat, CompiledChannel, UpstreamAuth};

    #[test]
    fn rejects_malformed_bearer_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Basic client-key"),
        );

        assert!(parse_bearer_token(&headers).is_err());
    }

    #[test]
    fn removes_static_and_connection_declared_hop_by_hop_request_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONNECTION,
            HeaderValue::from_bytes(b"x-internal-hop,\xff").unwrap(),
        );
        headers.insert("x-internal-hop", HeaderValue::from_static("discard"));
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-key"),
        );
        headers.insert("x-request-id", HeaderValue::from_static("keep"));

        let forwarded = forward_request_headers(&headers);
        assert!(forwarded.get(CONNECTION).is_none());
        assert!(forwarded.get("x-internal-hop").is_none());
        assert!(forwarded.get("authorization").is_none());
        assert_eq!(forwarded.get("x-request-id").unwrap(), "keep");
    }

    #[test]
    fn removes_static_and_connection_declared_hop_by_hop_response_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("x-upstream-hop"));
        headers.insert("x-upstream-hop", HeaderValue::from_static("discard"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.append("set-cookie", HeaderValue::from_static("first=value"));
        headers.append("set-cookie", HeaderValue::from_static("second=value"));

        let forwarded = forward_response_headers(&headers);
        assert!(forwarded.get(CONNECTION).is_none());
        assert!(forwarded.get("x-upstream-hop").is_none());
        assert!(forwarded.get("transfer-encoding").is_none());
        assert_eq!(forwarded.get_all("set-cookie").iter().count(), 2);
    }

    #[test]
    fn identifies_statuses_for_which_axum_will_not_poll_a_body() {
        assert!(response_has_no_body(StatusCode::NO_CONTENT));
        assert!(response_has_no_body(StatusCode::NOT_MODIFIED));
        assert!(response_has_no_body(StatusCode::CONTINUE));
        assert!(!response_has_no_body(StatusCode::OK));
    }

    #[test]
    fn injects_a_configured_custom_upstream_auth_header() {
        let channel = CompiledChannel::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiChatCompletions,
            Url::parse("https://example.test").unwrap(),
            1,
            UpstreamAuth::Header {
                name: HeaderName::from_static("x-api-key"),
                value: Arc::from("upstream-secret"),
            },
            HashSet::new(),
        );
        let mut headers = HeaderMap::new();
        inject_upstream_auth(&mut headers, &channel).unwrap();
        assert_eq!(headers.get("x-api-key").unwrap(), "upstream-secret");
        assert!(headers.get("authorization").is_none());
    }
}
