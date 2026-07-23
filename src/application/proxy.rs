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
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{
    admission::{AdmissionError, AdmissionLease, AdmissionRuntime},
    application::{
        AutomaticDisableService, ErrorKeywordMatcher, NoopRequestLogSink, RequestLogSink,
    },
    domain::{
        ApiFormat, ApiKeyPermission, AutomaticDisableSettings, AutomaticDisableTrigger,
        CompiledAdvancedBilling, CompiledApiKey, CompiledChannel, CompiledModelRule,
        ModelPriceSnapshot, RequestBilling, RequestLogEvent, RequestLogOutcome, RequestLogSource,
        RequestPriceSnapshot, RequestUsage, SessionAffinityKeySource, SessionAffinitySettings,
        UpstreamAuth,
    },
    routing::{
        ChannelLease, RoutingRuntime, SelectionResult, SessionAffinityMatch,
        SessionAffinitySelection,
    },
    runtime_config::RuntimeConfig,
    transforms::{
        SseEventPatchPlan, SseTransformer, apply_header_plan, apply_json_patch_plan,
        apply_response_header_plan, parse_connection_header_names,
    },
    upstream::{ResolvedUpstreamPolicy, UpstreamClientRegistry},
};

use super::usage::{SseTerminalOutcome, UsageCollector};

/// Data-plane use case backed by a single immutable configuration snapshot per
/// request and a process-shared upstream client registry.
#[derive(Clone)]
pub struct ProxyService {
    runtime: Arc<RuntimeConfig>,
    upstream_clients: Arc<UpstreamClientRegistry>,
    max_request_body_bytes: usize,
    request_log_sink: Arc<dyn RequestLogSink>,
    routing: RoutingRuntime,
    admission: AdmissionRuntime,
    automatic_disable: Option<AutomaticDisableService>,
}

impl ProxyService {
    pub fn new(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
    ) -> Result<Self, reqwest::Error> {
        Self::with_log_sink(
            runtime,
            max_request_body_bytes,
            Arc::new(NoopRequestLogSink),
        )
    }

    pub fn with_log_sink(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        request_log_sink: Arc<dyn RequestLogSink>,
    ) -> Result<Self, reqwest::Error> {
        Self::with_log_sink_and_routing(
            runtime,
            max_request_body_bytes,
            request_log_sink,
            RoutingRuntime::new(crate::routing::PassiveHealthPolicy::default()),
        )
    }

    pub fn with_log_sink_and_routing(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
    ) -> Result<Self, reqwest::Error> {
        Self::with_dependencies(
            runtime,
            max_request_body_bytes,
            request_log_sink,
            routing,
            AdmissionRuntime::new(),
        )
    }

    pub fn with_dependencies(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
    ) -> Result<Self, reqwest::Error> {
        Self::with_dependencies_and_registry(
            runtime,
            max_request_body_bytes,
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
        upstream_clients: Arc<UpstreamClientRegistry>,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
    ) -> Result<Self, reqwest::Error> {
        Self::with_dependencies_and_registry_and_automation(
            runtime,
            max_request_body_bytes,
            upstream_clients,
            request_log_sink,
            routing,
            admission,
            None,
        )
    }

    /// Adds asynchronous automatic-disable reporting to the proxy without
    /// allowing persistence to delay client-visible forwarding.
    pub fn with_dependencies_and_registry_and_automation(
        runtime: Arc<RuntimeConfig>,
        max_request_body_bytes: usize,
        upstream_clients: Arc<UpstreamClientRegistry>,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
        automatic_disable: Option<AutomaticDisableService>,
    ) -> Result<Self, reqwest::Error> {
        routing.reconcile(&runtime.snapshot());
        Ok(Self {
            runtime,
            upstream_clients,
            max_request_body_bytes,
            request_log_sink,
            routing,
            admission,
            automatic_disable,
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
        let session_affinity = match_session_affinity(
            snapshot.system_settings().session_affinity(),
            api_format,
            &parsed.model,
            &parts.headers,
            &original_body,
        );
        let route = match self.routing.select_with_affinity(
            &snapshot,
            &api_key,
            api_format,
            &parsed.model,
            session_affinity.clone(),
        ) {
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
        let crate::routing::SelectedRoute {
            rule,
            channel,
            session_affinity: selected_session_affinity,
            lease,
        } = route;
        let mut current_rule = rule;
        let mut current_channel = channel;
        let current_session_affinity = selected_session_affinity;
        let request_multiplier = request_billing_multiplier(&current_rule, &original_body);
        let mut completion = CompletionGuard::new(
            Arc::clone(&self.request_log_sink),
            &api_key,
            &parsed.model,
            parsed.streamed,
            api_format,
            &current_rule,
            &current_channel,
            lease,
            admission,
            started_wall_at,
            started_at,
            self.automatic_disable.clone(),
            snapshot.system_settings().automatic_disable().clone(),
            current_session_affinity.as_ref(),
            request_multiplier,
        );
        let retry_settings = snapshot.system_settings().request_retry();
        let max_retries = if retry_settings.enabled() {
            retry_settings.max_retries()
        } else {
            0
        };
        let max_attempts = max_retries.saturating_add(1);
        let mut attempt = 1_u32;
        let mut attempted_channel_ids = HashSet::with_capacity(max_attempts as usize);

        loop {
            attempted_channel_ids.insert(current_channel.id());
            let transforms = current_channel.upstream_policy().effective_transforms();
            let body =
                match rewrite_model_alias(original_body.clone(), &parsed.model, &current_rule) {
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

            // Apply the plan before hop-by-hop cleanup so `HeaderPlan` can
            // reject dynamically protected names declared by the client
            // `Connection` header. Cleanup then removes those names again.
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

            let url = match upstream_url(&current_channel, &parts.uri) {
                Ok(value) => value,
                Err(error) => {
                    completion.finish(RequestOutcome::UpstreamUnavailable);
                    return Err(error);
                }
            };
            if let Err(error) = inject_upstream_auth(&mut headers, &current_channel) {
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return Err(error);
            }
            let upstream_policy = match ResolvedUpstreamPolicy::try_resolve(
                &snapshot.system_settings().upstream_timeouts(),
                current_channel.upstream_policy(),
            ) {
                Ok(policy) => policy,
                Err(_) => {
                    completion.finish(RequestOutcome::UpstreamUnavailable);
                    return Err(ProxyError::upstream_unavailable());
                }
            };
            let upstream_client = match self
                .upstream_clients
                .client_for(current_channel.upstream_policy(), upstream_policy)
            {
                Ok(client) => client,
                Err(_) => {
                    completion.finish(RequestOutcome::UpstreamUnavailable);
                    return Err(ProxyError::upstream_unavailable());
                }
            };

            let upstream_request = upstream_client
                .request(parts.method.clone(), url)
                .headers(headers)
                .body(body);
            let send_result = match timeout(
                upstream_policy.timeouts().response_header(),
                upstream_request.send(),
            )
            .await
            {
                Err(_) => Err(PreHeaderFailure::ResponseHeaderTimeout),
                Ok(Err(error)) if error.is_timeout() && error.is_connect() => {
                    Err(PreHeaderFailure::ConnectTimeout)
                }
                Ok(Err(error)) if error.is_connect() => Err(PreHeaderFailure::ConnectionFailure),
                Ok(Err(_)) => {
                    completion.finish(RequestOutcome::UpstreamUnavailable);
                    return Err(ProxyError::upstream_unavailable());
                }
                Ok(Ok(response)) => Ok(response),
            };

            let upstream_response = match send_result {
                Ok(response) => {
                    completion.response_headers_received();
                    response
                }
                Err(failure) => {
                    failure.record_health(&mut completion);
                    let retry_route = (attempt < max_attempts).then(|| {
                        self.routing.select_with_affinity_excluding(
                            &snapshot,
                            &api_key,
                            api_format,
                            &parsed.model,
                            session_affinity.clone(),
                            &attempted_channel_ids,
                        )
                    });
                    let Some(SelectionResult::Selected(route)) = retry_route else {
                        completion.finish(failure.outcome());
                        return Err(failure.proxy_error());
                    };
                    let crate::routing::SelectedRoute {
                        rule,
                        channel,
                        session_affinity: selected_session_affinity,
                        lease,
                    } = route;
                    let failed_channel_id = current_channel.id();
                    let next_channel_id = channel.id();
                    let request_billing_multiplier =
                        request_billing_multiplier(&rule, &original_body);
                    completion.retry_with_route(
                        &rule,
                        &channel,
                        lease,
                        self.automatic_disable.clone(),
                        snapshot.system_settings().automatic_disable().clone(),
                        selected_session_affinity.as_ref(),
                        request_billing_multiplier,
                    );
                    current_rule = rule;
                    current_channel = channel;
                    attempt = attempt.saturating_add(1);
                    tracing::warn!(
                        event = "proxy_request_retry",
                        api_key_id = %api_key.id(),
                        client_model = %parsed.model,
                        failed_channel_id = %failed_channel_id,
                        next_channel_id = %next_channel_id,
                        attempt,
                        max_retries,
                        reason = failure.error_code(),
                        "retrying proxy request on another channel"
                    );
                    continue;
                }
            };

            return response_from_upstream(
                upstream_response,
                upstream_policy.timeouts().stream_idle(),
                completion,
                transforms.response_headers(),
                transforms.sse_event_patches().clone(),
            );
        }
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
            request_source: RequestLogSource::Client,
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
            billing: None,
            error_code: Some("model_not_found".into()),
            error_summary: None,
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
            request_source: RequestLogSource::Client,
            api_format,
            client_model: client_model.to_owned(),
            upstream_model: Some(rule.upstream_model().to_owned()),
            model_rule_id: Some(rule.id()),
            channel_group_id: None,
            channel_id: None,
            model_id: Some(rule.upstream_model_id()),
            outcome: RequestLogOutcome::Failed,
            response_status_code: Some(StatusCode::SERVICE_UNAVAILABLE.as_u16()),
            streamed,
            ttft_ms: None,
            total_duration_ms: clamp_duration_ms(started.elapsed()),
            billing: None,
            error_code: Some("no_healthy_channel".into()),
            error_summary: None,
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

fn request_billing_multiplier(rule: &CompiledModelRule, body: &[u8]) -> Decimal {
    let advanced_billing = rule.advanced_billing();
    if !advanced_billing.has_request_multipliers() {
        return Decimal::ONE;
    }
    let request =
        serde_json::from_slice::<Value>(body).expect("model probe already validated request JSON");
    advanced_billing.request_multiplier(&request)
}

const MAX_SESSION_AFFINITY_VALUE_BYTES: usize = 512;

fn match_session_affinity(
    settings: &SessionAffinitySettings,
    api_format: ApiFormat,
    model: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<SessionAffinityMatch> {
    if !settings.enabled() {
        return None;
    }
    let mut parsed_json: Option<Option<Value>> = None;
    for rule in settings.rules() {
        if !rule.matches_request(api_format, model) {
            continue;
        }
        for source in rule.key_sources() {
            let value = match source {
                SessionAffinityKeySource::RequestHeader(name) => headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                SessionAffinityKeySource::JsonPointer(pointer) => {
                    let parsed = parsed_json
                        .get_or_insert_with(|| serde_json::from_slice::<Value>(body).ok());
                    parsed
                        .as_ref()
                        .and_then(|value| value.pointer(pointer))
                        .and_then(session_affinity_scalar)
                }
            };
            let Some(value) = value.filter(|value| {
                !value.is_empty() && value.len() <= MAX_SESSION_AFFINITY_VALUE_BYTES
            }) else {
                continue;
            };
            if !rule.matches_value(&value) {
                continue;
            }
            let session_hash = Sha256::digest(value.as_bytes()).into();
            return Some(SessionAffinityMatch::new(
                Arc::from(rule.name()),
                rule.fingerprint(),
                session_hash,
                rule.ttl(),
            ));
        }
    }
    None
}

fn session_affinity_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        }
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
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
    completion.configure_usage_collector(is_sse_response(original_upstream_headers));
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
                        let default_outcome = completed_transport_outcome(state.upstream_succeeded);
                        if let Some(transformer) = &mut state.sse_transformer {
                            if let Some(residual) = transformer.finish() {
                                record_stream_bytes(&mut state, &residual);
                                let outcome = state
                                    .completion
                                    .finalize_usage()
                                    .map(|terminal| {
                                        sse_terminal_request_outcome(
                                            terminal,
                                            state.upstream_succeeded,
                                        )
                                    })
                                    .unwrap_or(default_outcome);
                                state.completion.finish(outcome);
                                return Some((Ok(residual), state));
                            }
                        }
                        let outcome = state
                            .completion
                            .finalize_usage()
                            .map(|terminal| {
                                sse_terminal_request_outcome(terminal, state.upstream_succeeded)
                            })
                            .unwrap_or(default_outcome);
                        state.completion.finish(outcome);
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
        state.completion.observe_upstream_error_body(bytes);
        state.completion.record_first_byte();
        if let Some(terminal) = state.completion.observe_usage(bytes) {
            state.completion.finish(sse_terminal_request_outcome(
                terminal,
                state.upstream_succeeded,
            ));
        }
    }
    if let Some(remaining) = &mut state.remaining_bytes {
        *remaining = remaining.saturating_sub(bytes.len() as u64);
        if *remaining == 0 {
            let outcome = state
                .completion
                .finalize_usage()
                .map(|terminal| sse_terminal_request_outcome(terminal, state.upstream_succeeded))
                .unwrap_or_else(|| completed_transport_outcome(state.upstream_succeeded));
            state.completion.finish(outcome);
        }
    }
}

const fn completed_transport_outcome(upstream_succeeded: bool) -> RequestOutcome {
    if upstream_succeeded {
        RequestOutcome::Succeeded
    } else {
        RequestOutcome::UpstreamHttpError
    }
}

const fn sse_terminal_request_outcome(
    terminal: SseTerminalOutcome,
    upstream_succeeded: bool,
) -> RequestOutcome {
    match (terminal, upstream_succeeded) {
        (SseTerminalOutcome::Completed, true) => RequestOutcome::Succeeded,
        (SseTerminalOutcome::Failed, true) => RequestOutcome::UpstreamSseError,
        (SseTerminalOutcome::Completed | SseTerminalOutcome::Failed, false) => {
            RequestOutcome::UpstreamHttpError
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
enum PreHeaderFailure {
    ConnectionFailure,
    ConnectTimeout,
    ResponseHeaderTimeout,
}

impl PreHeaderFailure {
    const fn outcome(self) -> RequestOutcome {
        match self {
            Self::ConnectionFailure => RequestOutcome::UpstreamUnavailable,
            Self::ConnectTimeout => RequestOutcome::ConnectTimeout,
            Self::ResponseHeaderTimeout => RequestOutcome::ResponseHeaderTimeout,
        }
    }

    fn proxy_error(self) -> ProxyError {
        match self {
            Self::ConnectionFailure => ProxyError::upstream_unavailable(),
            Self::ConnectTimeout => ProxyError::connect_timeout(),
            Self::ResponseHeaderTimeout => ProxyError::response_header_timeout(),
        }
    }

    const fn error_code(self) -> &'static str {
        match self {
            Self::ConnectionFailure => "upstream_unavailable",
            Self::ConnectTimeout => "connect_timeout",
            Self::ResponseHeaderTimeout => "response_header_timeout",
        }
    }

    fn record_health(self, completion: &mut CompletionGuard) {
        match self {
            Self::ConnectionFailure | Self::ConnectTimeout => completion.connection_failed(),
            Self::ResponseHeaderTimeout => completion.probe_failed(),
        }
    }
}

#[derive(Clone, Copy)]
enum RequestOutcome {
    Succeeded,
    UpstreamHttpError,
    UpstreamSseError,
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
            Self::UpstreamSseError => "upstream_sse_error",
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
            | Self::UpstreamSseError
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
            Self::UpstreamSseError => Some("upstream_sse_error"),
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
            | Self::UpstreamSseError
            | Self::UpstreamBodyError
            | Self::StreamIdleTimeout
            | Self::Cancelled => None,
        }
    }

    const fn evicts_session_affinity(self) -> bool {
        matches!(
            self,
            Self::UpstreamHttpError
                | Self::UpstreamSseError
                | Self::ConnectTimeout
                | Self::ResponseHeaderTimeout
                | Self::UpstreamUnavailable
                | Self::UpstreamBodyError
                | Self::StreamIdleTimeout
        )
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
    usage: UsageCollector,
    price_snapshot: ModelPriceSnapshot,
    advanced_billing: CompiledAdvancedBilling,
    billing_multiplier: Decimal,
    request_billing_multiplier: Decimal,
    sink: Arc<dyn RequestLogSink>,
    session_affinity_rule: Option<Arc<str>>,
    session_affinity_hit: Option<bool>,
    attempts: u32,
}

/// Emits exactly one event, including when Axum drops an in-flight response
/// body after a downstream client disconnects.
struct CompletionGuard {
    context: Option<CompletionContext>,
    lease: Option<ChannelLease>,
    _admission: Option<AdmissionLease>,
    automatic_disable: Option<AutomaticDisableContext>,
}

struct AutomaticDisableContext {
    channel_id: Uuid,
    settings: AutomaticDisableSettings,
    service: AutomaticDisableService,
    keyword_matcher: Option<ErrorKeywordMatcher>,
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
        automatic_disable_service: Option<AutomaticDisableService>,
        automatic_disable_settings: AutomaticDisableSettings,
        session_affinity: Option<&SessionAffinitySelection>,
        request_billing_multiplier: Decimal,
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
                model_id: rule.upstream_model_id(),
                api_format,
                streamed,
                started_wall_at,
                started_at,
                first_byte_at: None,
                upstream_status: None,
                client_visible_status: None,
                usage: UsageCollector::new(api_format, false),
                price_snapshot: rule.price_snapshot().clone(),
                advanced_billing: rule.advanced_billing().clone(),
                billing_multiplier: channel.billing_multiplier(),
                request_billing_multiplier,
                sink,
                session_affinity_rule: session_affinity
                    .map(|selection| Arc::from(selection.rule_name())),
                session_affinity_hit: session_affinity.map(SessionAffinitySelection::cache_hit),
                attempts: 1,
            }),
            lease: Some(lease),
            _admission: Some(admission),
            automatic_disable: automatic_disable_context(
                channel,
                automatic_disable_service,
                automatic_disable_settings,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn retry_with_route(
        &mut self,
        rule: &CompiledModelRule,
        channel: &CompiledChannel,
        lease: ChannelLease,
        automatic_disable_service: Option<AutomaticDisableService>,
        automatic_disable_settings: AutomaticDisableSettings,
        session_affinity: Option<&SessionAffinitySelection>,
        request_billing_multiplier: Decimal,
    ) {
        if let Some(mut previous) = self.lease.take() {
            previous.request_failed();
        }
        if let Some(context) = &mut self.context {
            context.upstream_model = rule.upstream_model().to_owned();
            context.model_rule_id = rule.id();
            context.channel_group_id = channel.group_id();
            context.channel_id = channel.id();
            context.model_id = rule.upstream_model_id();
            context.first_byte_at = None;
            context.upstream_status = None;
            context.client_visible_status = None;
            context.usage = UsageCollector::new(context.api_format, false);
            context.price_snapshot = rule.price_snapshot().clone();
            context.advanced_billing = rule.advanced_billing().clone();
            context.billing_multiplier = channel.billing_multiplier();
            context.request_billing_multiplier = request_billing_multiplier;
            context.session_affinity_rule =
                session_affinity.map(|selection| Arc::from(selection.rule_name()));
            context.session_affinity_hit =
                session_affinity.map(SessionAffinitySelection::cache_hit);
            context.attempts = context.attempts.saturating_add(1);
        }
        self.lease = Some(lease);
        self.automatic_disable = automatic_disable_context(
            channel,
            automatic_disable_service,
            automatic_disable_settings,
        );
    }

    fn set_upstream_status(&mut self, status: u16) {
        if let Some(context) = &mut self.context {
            context.upstream_status = Some(status);
        }
        if (StatusCode::OK.as_u16()..StatusCode::MULTIPLE_CHOICES.as_u16()).contains(&status) {
            return;
        }
        let Some(automatic_disable) = &mut self.automatic_disable else {
            return;
        };
        if automatic_disable.settings.matches_status(status) {
            automatic_disable.service.try_report(
                automatic_disable.channel_id,
                AutomaticDisableTrigger::HttpStatus(status),
            );
        }
        automatic_disable.keyword_matcher = ErrorKeywordMatcher::new(&automatic_disable.settings);
    }

    fn set_client_visible_status(&mut self, status: u16) {
        if let Some(context) = &mut self.context {
            context.client_visible_status = Some(status);
        }
    }

    fn configure_usage_collector(&mut self, sse: bool) {
        if let Some(context) = &mut self.context {
            context.usage = UsageCollector::new(context.api_format, sse);
        }
    }

    fn observe_usage(&mut self, bytes: &Bytes) -> Option<SseTerminalOutcome> {
        if let Some(context) = &mut self.context {
            context.usage.observe(bytes);
            context.usage.sse_terminal_outcome()
        } else {
            None
        }
    }

    fn observe_upstream_error_body(&mut self, bytes: &Bytes) {
        let Some(automatic_disable) = &mut self.automatic_disable else {
            return;
        };
        let Some(matcher) = &mut automatic_disable.keyword_matcher else {
            return;
        };
        if let Some(trigger) = matcher.observe(bytes) {
            automatic_disable
                .service
                .try_report(automatic_disable.channel_id, trigger);
            automatic_disable.keyword_matcher = None;
        }
    }

    fn finalize_usage(&mut self) -> Option<SseTerminalOutcome> {
        if let Some(context) = &mut self.context {
            context.usage.finalize();
            context.usage.sse_terminal_outcome()
        } else {
            None
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
        if let Some(lease) = &mut self.lease {
            if matches!(outcome, RequestOutcome::Succeeded) {
                lease.request_succeeded();
            } else if outcome.evicts_session_affinity() {
                lease.request_failed();
            }
        }
        let usage = context.usage.latest();
        let upstream_error = context.usage.sse_error().cloned();
        let total_duration_ms = clamp_duration_ms(context.started_at.elapsed());
        let billing = request_billing(
            &context.price_snapshot,
            &context.advanced_billing,
            context.billing_multiplier,
            context.request_billing_multiplier,
            usage,
            total_duration_ms,
            context.first_byte_at.map(clamp_duration_ms),
        );
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
            input_tokens = ?usage.map(|usage| usage.input_tokens),
            output_tokens = ?usage.map(|usage| usage.output_tokens),
            session_affinity_rule = ?context.session_affinity_rule,
            session_affinity_hit = ?context.session_affinity_hit,
            attempts = context.attempts,
            outcome = outcome.tracing_outcome(),
            "proxy request completed"
        );
        let event = RequestLogEvent {
            id: context.event_id,
            started_at: context.started_wall_at,
            completed_at: completed_at(context.started_wall_at, context.started_at.elapsed()),
            user_id: context.user_id,
            api_key_id: context.api_key_id,
            request_source: RequestLogSource::Client,
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
            billing: Some(billing),
            error_code: upstream_error
                .as_ref()
                .and_then(|error| error.code.clone())
                .or_else(|| outcome.error_code().map(str::to_owned)),
            error_summary: upstream_error.and_then(|error| error.summary),
        };
        context.sink.try_record(event);
    }
}

fn automatic_disable_context(
    channel: &CompiledChannel,
    automatic_disable_service: Option<AutomaticDisableService>,
    automatic_disable_settings: AutomaticDisableSettings,
) -> Option<AutomaticDisableContext> {
    channel
        .auto_disable_allowed()
        .then(|| {
            automatic_disable_service.map(|service| AutomaticDisableContext {
                channel_id: channel.id(),
                settings: automatic_disable_settings,
                service,
                keyword_matcher: None,
            })
        })
        .flatten()
}

fn request_billing(
    snapshot: &ModelPriceSnapshot,
    advanced_billing: &CompiledAdvancedBilling,
    billing_multiplier: Decimal,
    request_billing_multiplier: Decimal,
    usage: Option<super::usage::ResponseUsage>,
    total_duration_ms: i32,
    ttft_ms: Option<i32>,
) -> RequestBilling {
    let usage = usage.map(|usage| RequestUsage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        output_tokens: usage.output_tokens,
    });
    let (input_unit_price, cached_input_unit_price, cache_write_unit_price) =
        usage.as_ref().map_or(
            (
                snapshot.input_unit_price(),
                snapshot.cached_input_unit_price(),
                snapshot.cache_write_unit_price(),
            ),
            |usage| {
                advanced_billing.input_prices(
                    usage.input_tokens,
                    snapshot.input_unit_price(),
                    snapshot.cached_input_unit_price(),
                    snapshot.cache_write_unit_price(),
                )
            },
        );
    let billing_multiplier = billing_multiplier
        .checked_mul(request_billing_multiplier)
        .expect("compiled request billing multiplier fits");
    let price = RequestPriceSnapshot {
        currency: snapshot.currency().to_owned(),
        price_unit_tokens: snapshot.price_unit_tokens(),
        price_effective_at: snapshot.price_effective_at(),
        input_unit_price: effective_unit_price(input_unit_price, billing_multiplier),
        cached_input_unit_price: effective_unit_price(cached_input_unit_price, billing_multiplier),
        cache_write_unit_price: effective_unit_price(cache_write_unit_price, billing_multiplier),
        output_unit_price: effective_unit_price(snapshot.output_unit_price(), billing_multiplier),
    };
    let cost_amount = usage.as_ref().map(|usage| calculate_cost(usage, &price));
    let output_tokens_per_second = usage.and_then(|usage| {
        (usage.output_tokens > 0).then(|| {
            let generation_ms = total_duration_ms
                .saturating_sub(ttft_ms.unwrap_or(0))
                .max(1);
            (Decimal::from(usage.output_tokens) * Decimal::from(1_000_i64)
                / Decimal::from(generation_ms))
            .round_dp(4)
        })
    });
    RequestBilling {
        usage,
        price,
        cost_amount,
        output_tokens_per_second,
    }
}

fn effective_unit_price(price: Decimal, billing_multiplier: Decimal) -> Decimal {
    price
        .checked_mul(billing_multiplier)
        .expect("compiled channel billing price multiplication fits")
        .round_dp(12)
}

fn calculate_cost(usage: &RequestUsage, price: &RequestPriceSnapshot) -> Decimal {
    let unit = Decimal::from(price.price_unit_tokens);
    let non_cached_input = usage.input_tokens.saturating_sub(usage.cached_input_tokens);
    ((Decimal::from(non_cached_input) * price.input_unit_price
        + Decimal::from(usage.cached_input_tokens) * price.cached_input_unit_price
        + Decimal::from(usage.cache_write_tokens) * price.cache_write_unit_price
        + Decimal::from(usage.output_tokens) * price.output_unit_price)
        / unit)
        .round_dp(8)
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
    use std::{collections::HashSet, sync::Arc, time::Duration};

    use regex::Regex;
    use reqwest::{Url, header::HeaderName};
    use rust_decimal::Decimal;
    use uuid::Uuid;

    use super::{
        calculate_cost, forward_request_headers, forward_response_headers, inject_upstream_auth,
        match_session_affinity, parse_bearer_token, request_billing, response_has_no_body,
    };
    use crate::{
        application::usage::ResponseUsage,
        domain::{
            AdvancedBilling, ApiFormat, CompiledAdvancedBilling, CompiledChannel, LongContextTier,
            ModelPriceSnapshot, RequestBillingMultiplier, RequestPriceSnapshot, RequestUsage,
            SessionAffinityKeySource, SessionAffinityRule, SessionAffinitySettings, UpstreamAuth,
        },
    };
    use serde_json::json;

    fn affinity_settings(sources: Vec<SessionAffinityKeySource>) -> SessionAffinitySettings {
        SessionAffinitySettings::new(
            true,
            100,
            Duration::from_secs(60),
            vec![SessionAffinityRule::new(
                Arc::from("test"),
                [1; 32],
                vec![ApiFormat::OpenAiResponses].into(),
                vec![Regex::new("^gpt-.*$").unwrap()].into(),
                sources.into(),
                Some(Regex::new("^session-").unwrap()),
                Duration::from_secs(60),
            )]
            .into(),
        )
    }

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
    fn extracts_session_affinity_from_ordered_header_and_json_sources() {
        let settings = affinity_settings(vec![
            SessionAffinityKeySource::RequestHeader(HeaderName::from_static("x-session-id")),
            SessionAffinityKeySource::JsonPointer(Arc::from("/prompt_cache_key")),
        ]);
        let headers = HeaderMap::new();
        let matched = match_session_affinity(
            &settings,
            ApiFormat::OpenAiResponses,
            "gpt-5",
            &headers,
            br#"{"model":"gpt-5","prompt_cache_key":"session-body"}"#,
        );
        assert!(matched.is_some());
    }

    #[test]
    fn ignores_non_scalar_or_overlong_session_affinity_values() {
        let settings = affinity_settings(vec![SessionAffinityKeySource::JsonPointer(Arc::from(
            "/metadata",
        ))]);
        let headers = HeaderMap::new();
        assert!(
            match_session_affinity(
                &settings,
                ApiFormat::OpenAiResponses,
                "gpt-5",
                &headers,
                br#"{"model":"gpt-5","metadata":{"session":"session-object"}}"#,
            )
            .is_none()
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-session-id",
            HeaderValue::from_str(&format!("session-{}", "x".repeat(600))).unwrap(),
        );
        let header_settings = affinity_settings(vec![SessionAffinityKeySource::RequestHeader(
            HeaderName::from_static("x-session-id"),
        )]);
        assert!(
            match_session_affinity(
                &header_settings,
                ApiFormat::OpenAiResponses,
                "gpt-5",
                &headers,
                br#"{"model":"gpt-5"}"#,
            )
            .is_none()
        );
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

    #[test]
    fn bills_cached_input_cache_writes_and_output_from_the_selected_snapshot() {
        let price = RequestPriceSnapshot {
            currency: "USD".into(),
            price_unit_tokens: 1,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Decimal::ONE,
            cached_input_unit_price: Decimal::new(5, 1),
            cache_write_unit_price: Decimal::new(25, 2),
            output_unit_price: Decimal::from(2_i64),
        };
        let usage = RequestUsage {
            input_tokens: 10,
            cached_input_tokens: 2,
            cache_write_tokens: 1,
            output_tokens: 4,
        };
        assert_eq!(calculate_cost(&usage, &price), Decimal::new(1725, 2));

        let snapshot = ModelPriceSnapshot::new(
            "USD".into(),
            1,
            price.price_effective_at,
            price.input_unit_price,
            price.cached_input_unit_price,
            price.cache_write_unit_price,
            price.output_unit_price,
        );
        let billing = request_billing(
            &snapshot,
            &CompiledAdvancedBilling::default(),
            Decimal::ONE,
            Decimal::ONE,
            Some(ResponseUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
            }),
            2_000,
            Some(500),
        );
        assert_eq!(billing.cost_amount, Some(Decimal::new(1725, 2)));
        assert_eq!(
            billing.output_tokens_per_second,
            Some(Decimal::new(26667, 4))
        );
    }

    #[test]
    fn applies_the_selected_channel_billing_multiplier_to_price_and_cost() {
        let snapshot = ModelPriceSnapshot::new(
            "USD".into(),
            1,
            chrono::Utc::now(),
            Decimal::ONE,
            Decimal::new(5, 1),
            Decimal::new(25, 2),
            Decimal::from(2_i64),
        );
        let billing = request_billing(
            &snapshot,
            &CompiledAdvancedBilling::default(),
            Decimal::new(15, 1),
            Decimal::ONE,
            Some(ResponseUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
            }),
            2_000,
            Some(500),
        );

        assert_eq!(billing.price.input_unit_price, Decimal::new(15, 1));
        assert_eq!(billing.price.cached_input_unit_price, Decimal::new(75, 2));
        assert_eq!(billing.price.cache_write_unit_price, Decimal::new(375, 3));
        assert_eq!(billing.price.output_unit_price, Decimal::from(3_i64));
        assert_eq!(billing.cost_amount, Some(Decimal::new(25875, 3)));
    }

    #[test]
    fn applies_context_tier_then_channel_and_request_multipliers() {
        let snapshot = ModelPriceSnapshot::new(
            "USD".into(),
            1,
            chrono::Utc::now(),
            Decimal::ONE,
            Decimal::new(5, 1),
            Decimal::new(25, 2),
            Decimal::from(2_i64),
        );
        let advanced = CompiledAdvancedBilling::compile(AdvancedBilling {
            long_context_tiers: vec![LongContextTier {
                input_tokens_threshold: 10,
                input_unit_price: Decimal::from(3_i64),
                cached_input_unit_price: Decimal::from(2_i64),
                cache_write_unit_price: Decimal::from(4_i64),
            }],
            request_multipliers: vec![RequestBillingMultiplier {
                json_pointer: "/reasoning/effort".into(),
                value: json!("high"),
                multiplier: Decimal::new(2, 0),
            }],
        })
        .unwrap();
        let billing = request_billing(
            &snapshot,
            &advanced,
            Decimal::new(15, 1),
            advanced.request_multiplier(&json!({"reasoning": {"effort": "high"}})),
            Some(ResponseUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
            }),
            2_000,
            Some(500),
        );

        assert_eq!(billing.price.input_unit_price, Decimal::from(9_i64));
        assert_eq!(billing.price.cached_input_unit_price, Decimal::from(6_i64));
        assert_eq!(billing.price.cache_write_unit_price, Decimal::from(12_i64));
        assert_eq!(billing.price.output_unit_price, Decimal::from(6_i64));
        assert_eq!(billing.cost_amount, Some(Decimal::from(120_i64)));
    }
}
