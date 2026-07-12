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
        header::{AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HOST, PROXY_AUTHORIZATION},
    },
    response::{IntoResponse, Response as AxumResponse},
};
use futures_util::{Stream, StreamExt, stream};
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    application::{NoopRequestLogSink, RequestLogSink},
    domain::{
        ApiFormat, ApiKeyPermission, CompiledApiKey, CompiledChannel, CompiledModelRule,
        RequestLogEvent, RequestLogOutcome, UpstreamAuth,
    },
    routing::{ChannelLease, RoutingRuntime, SelectionResult},
    runtime_config::{RuntimeConfig, UpstreamConfig},
};

/// Data-plane use case backed by a single immutable configuration snapshot per
/// request and a reusable reqwest client.
#[derive(Clone)]
pub struct ProxyService {
    runtime: Arc<RuntimeConfig>,
    upstream_client: Client,
    max_request_body_bytes: usize,
    response_header_timeout: Duration,
    stream_idle_timeout: Duration,
    request_log_sink: Arc<dyn RequestLogSink>,
    routing: RoutingRuntime,
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
        let upstream_client = Client::builder()
            .connect_timeout(Duration::from_secs(upstream.connect_timeout_seconds))
            .redirect(Policy::none())
            .build()?;

        Ok(Self {
            runtime,
            upstream_client,
            max_request_body_bytes,
            response_header_timeout: Duration::from_secs(upstream.response_header_timeout_seconds),
            stream_idle_timeout: Duration::from_secs(upstream.stream_idle_timeout_seconds),
            request_log_sink,
            routing,
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
            started_wall_at,
            started_at,
        );
        let body = match rewrite_model_alias(original_body, &parsed.model, &route.rule) {
            Ok(value) => value,
            Err(error) => {
                completion.finish(RequestOutcome::ClientRequestError);
                return Err(error);
            }
        };

        let url = match upstream_url(&route.channel, &parts.uri) {
            Ok(value) => value,
            Err(error) => {
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return Err(error);
            }
        };
        let mut headers = forward_request_headers(&parts.headers);
        if let Err(error) = inject_upstream_auth(&mut headers, &route.channel) {
            completion.finish(RequestOutcome::UpstreamUnavailable);
            return Err(error);
        }

        let upstream_request = self
            .upstream_client
            .request(parts.method, url)
            .headers(headers)
            .body(body);
        let upstream_response =
            match timeout(self.response_header_timeout, upstream_request.send()).await {
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

        Ok(response_from_upstream(
            upstream_response,
            self.stream_idle_timeout,
            completion,
        ))
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
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
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
) -> AxumResponse {
    let upstream_status = upstream_response.status();
    let status = StatusCode::from_u16(upstream_status.as_u16())
        .expect("reqwest status code is valid for an Axum response");
    completion.set_upstream_status(upstream_status.as_u16());
    let headers = forward_response_headers(upstream_response.headers());
    let expected_body_bytes = upstream_response.content_length();
    if response_has_no_body(status) || expected_body_bytes == Some(0) {
        completion.finish(if upstream_status.is_success() {
            RequestOutcome::Succeeded
        } else {
            RequestOutcome::UpstreamHttpError
        });
        let mut response = Response::new(Body::empty());
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        return response;
    }
    let stream = timed_upstream_stream(
        upstream_response.bytes_stream(),
        stream_idle_timeout,
        completion,
        upstream_status.is_success(),
        expected_body_bytes,
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
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
}

fn timed_upstream_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    idle_timeout: Duration,
    completion: CompletionGuard,
    upstream_succeeded: bool,
    remaining_bytes: Option<u64>,
) -> impl Stream<Item = Result<Bytes, BodyStreamError>> + Send {
    stream::unfold(
        StreamState {
            upstream: Some(Box::pin(upstream)),
            idle_timeout,
            completion,
            upstream_succeeded,
            remaining_bytes,
        },
        |mut state| async move {
            let next = match state.upstream.as_mut() {
                Some(upstream) => timeout(state.idle_timeout, upstream.next()).await,
                None => return None,
            };

            match next {
                Ok(Some(Ok(bytes))) => {
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
                    Some((Ok(bytes), state))
                }
                Ok(Some(Err(error))) => {
                    state.upstream.take();
                    state.completion.finish(RequestOutcome::UpstreamBodyError);
                    let error: BodyStreamError = Box::new(error);
                    Some((Err(error), state))
                }
                Ok(None) => {
                    state.completion.finish(if state.upstream_succeeded {
                        RequestOutcome::Succeeded
                    } else {
                        RequestOutcome::UpstreamHttpError
                    });
                    None
                }
                Err(_) => {
                    state.upstream.take();
                    state.completion.finish(RequestOutcome::StreamIdleTimeout);
                    let error: BodyStreamError = Box::new(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "upstream response stream was idle for too long",
                    ));
                    Some((Err(error), state))
                }
            }
        },
    )
}

#[derive(Clone, Copy)]
enum RequestOutcome {
    Succeeded,
    UpstreamHttpError,
    ConnectTimeout,
    ResponseHeaderTimeout,
    UpstreamUnavailable,
    UpstreamBodyError,
    StreamIdleTimeout,
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
    sink: Arc<dyn RequestLogSink>,
}

/// Emits exactly one event, including when Axum drops an in-flight response
/// body after a downstream client disconnects.
struct CompletionGuard {
    context: Option<CompletionContext>,
    lease: Option<ChannelLease>,
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
                sink,
            }),
            lease: Some(lease),
        }
    }

    fn set_upstream_status(&mut self, status: u16) {
        if let Some(context) = &mut self.context {
            context.upstream_status = Some(status);
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
            response_status_code: context.upstream_status.or(outcome.fallback_status()),
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
        headers.insert(CONNECTION, HeaderValue::from_static("x-internal-hop"));
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
