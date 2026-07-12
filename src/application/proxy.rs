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

use crate::{
    domain::{ApiFormat, ApiKeyPermission, CompiledChannel, CompiledModelRule},
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
}

impl ProxyService {
    pub fn new(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream: &UpstreamConfig,
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
        })
    }

    pub async fn proxy(
        &self,
        api_format: ApiFormat,
        request: Request<Body>,
    ) -> Result<AxumResponse, ProxyError> {
        let started_at = Instant::now();
        let (parts, body) = request.into_parts();
        let client_key = parse_bearer_token(&parts.headers)?;
        let snapshot = self.runtime.snapshot();
        let api_key = snapshot
            .authenticate(client_key)
            .ok_or_else(ProxyError::invalid_api_key)?;
        if !api_key.permits(api_format, ApiKeyPermission::Proxy) {
            return Err(ProxyError::forbidden(
                "This API key cannot proxy requests in this API format.",
            ));
        }

        let original_body = to_bytes(body, self.max_request_body_bytes)
            .await
            .map_err(request_body_error)?;
        let model = parse_model(&original_body)?;
        let rule = snapshot
            .model_rule(api_format, &model)
            .ok_or_else(|| ProxyError::unknown_model(&model))?;
        let body = rewrite_model_alias(original_body, &model, &rule)?;

        let url = upstream_url(rule.channel(), &parts.uri)?;
        let mut headers = forward_request_headers(&parts.headers);
        inject_upstream_auth(&mut headers, rule.channel())?;
        let mut completion = CompletionGuard::new(
            api_key.id(),
            &model,
            rule.upstream_model(),
            rule.channel().id(),
            api_format,
            started_at,
        );

        let upstream_request = self
            .upstream_client
            .request(parts.method, url)
            .headers(headers)
            .body(body);
        let upstream_response =
            match timeout(self.response_header_timeout, upstream_request.send()).await {
                Err(_) => {
                    completion.finish(RequestOutcome::ResponseHeaderTimeout);
                    return Err(ProxyError::response_header_timeout());
                }
                Ok(Err(error)) => {
                    if error.is_timeout() && error.is_connect() {
                        completion.finish(RequestOutcome::ConnectTimeout);
                        return Err(ProxyError::connect_timeout());
                    }
                    completion.finish(RequestOutcome::UpstreamUnavailable);
                    return Err(ProxyError::upstream_unavailable());
                }
                Ok(Ok(response)) => response,
            };

        Ok(response_from_upstream(
            upstream_response,
            self.stream_idle_timeout,
            completion,
        ))
    }

    pub fn list_models(&self, headers: &HeaderMap) -> Result<ModelsResponse, ProxyError> {
        let client_key = parse_bearer_token(headers)?;
        let snapshot = self.runtime.snapshot();
        let api_key = snapshot
            .authenticate(client_key)
            .ok_or_else(ProxyError::invalid_api_key)?;

        let models = [ApiFormat::OpenAiChatCompletions, ApiFormat::OpenAiResponses]
            .into_iter()
            .flat_map(|api_format| snapshot.models_for(&api_key, api_format))
            .collect::<BTreeSet<_>>();
        if models.is_empty() {
            return Err(ProxyError::forbidden(
                "This API key cannot list models in any API format.",
            ));
        }

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
struct ModelProbe {
    model: String,
}

fn parse_model(body: &[u8]) -> Result<String, ProxyError> {
    let model = serde_json::from_slice::<ModelProbe>(body)
        .map_err(|_| {
            ProxyError::invalid_request("Request body must contain a string model.", "model")
        })?
        .model;
    if model.trim().is_empty() {
        return Err(ProxyError::invalid_request(
            "Request body must contain a non-empty model.",
            "model",
        ));
    }
    Ok(model)
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
    if let Some(token) = channel.upstream_auth().bearer_token() {
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| ProxyError {
            status: StatusCode::BAD_GATEWAY,
            message: "The selected upstream channel has invalid credentials.".to_owned(),
            error_type: "api_error",
            param: None,
            code: "invalid_upstream_credentials".into(),
            authenticate: false,
        })?;
        headers.insert(AUTHORIZATION, value);
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
    if response_has_no_body(status) {
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
}

fn timed_upstream_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    idle_timeout: Duration,
    completion: CompletionGuard,
    upstream_succeeded: bool,
) -> impl Stream<Item = Result<Bytes, BodyStreamError>> + Send {
    stream::unfold(
        StreamState {
            upstream: Some(Box::pin(upstream)),
            idle_timeout,
            completion,
            upstream_succeeded,
        },
        |mut state| async move {
            let next = match state.upstream.as_mut() {
                Some(upstream) => timeout(state.idle_timeout, upstream.next()).await,
                None => return None,
            };

            match next {
                Ok(Some(Ok(bytes))) => {
                    state.completion.record_first_byte();
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
}

impl RequestOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::UpstreamHttpError => "upstream_http_error",
            Self::ConnectTimeout => "connect_timeout",
            Self::ResponseHeaderTimeout => "response_header_timeout",
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::UpstreamBodyError => "upstream_body_error",
            Self::StreamIdleTimeout => "stream_idle_timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

struct CompletionContext {
    api_key_id: Arc<str>,
    client_model: Arc<str>,
    upstream_model: Arc<str>,
    channel_id: Arc<str>,
    api_format: ApiFormat,
    started_at: Instant,
    first_byte_at: Option<Duration>,
    upstream_status: Option<u16>,
}

/// Emits exactly one event, including when Axum drops an in-flight response
/// body after a downstream client disconnects.
struct CompletionGuard {
    context: Option<CompletionContext>,
}

impl CompletionGuard {
    fn new(
        api_key_id: &str,
        client_model: &str,
        upstream_model: &str,
        channel_id: &str,
        api_format: ApiFormat,
        started_at: Instant,
    ) -> Self {
        Self {
            context: Some(CompletionContext {
                api_key_id: Arc::from(api_key_id),
                client_model: Arc::from(client_model),
                upstream_model: Arc::from(upstream_model),
                channel_id: Arc::from(channel_id),
                api_format,
                started_at,
                first_byte_at: None,
                upstream_status: None,
            }),
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
            outcome = outcome.as_str(),
            "proxy request completed"
        );
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.finish(RequestOutcome::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header::CONNECTION};

    use super::{
        forward_request_headers, forward_response_headers, parse_bearer_token, response_has_no_body,
    };

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
}
