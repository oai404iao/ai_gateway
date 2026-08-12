mod websocket;

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
        HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode,
        header::{
            ACCEPT_ENCODING, ACCEPT_RANGES, AUTHORIZATION, CONNECTION, CONTENT_ENCODING,
            CONTENT_LENGTH, CONTENT_TYPE, HOST, PROXY_AUTHORIZATION, RANGE,
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
        AutomaticDisableService, ConnectorAttemptError, ConnectorUnavailable, ErrorKeywordMatcher,
        NoopRequestLogSink, RequestLogSink, UpstreamConnectorRegistry,
    },
    domain::{
        ApiFormat, ApiKeyPermission, ApiOperation, AutomaticDisableSettings,
        AutomaticDisableTrigger, CompiledAdvancedBilling, CompiledApiKey, CompiledChannel,
        CompiledModelRule, MAX_REQUEST_RETRIES, ModelPriceSnapshot, RequestLogEvent,
        RequestLogOutcome, RequestLogSource, RequestProtocol, SessionAffinityKeySource,
        SessionAffinitySettings,
    },
    request_policy::{
        RequestInterface, RequestPolicyError, RequestPolicyLayer, client_header_explicitly_ignored,
        filter_client_headers, strip_explicitly_ignored_client_headers,
    },
    routing::{
        ChannelLease, RoutingRuntime, SelectionResult, SessionAffinityMatch,
        SessionAffinitySelection,
    },
    runtime_config::RuntimeConfig,
    transforms::{
        JsonPatchPlan, SseEventPatchPlan, SseTransformer, apply_header_plan, apply_json_patch_plan,
        apply_response_header_plan, parse_connection_header_names,
    },
    upstream::{
        DecodedBodyError, ResolvedUpstreamPolicy, ResponseContentCodings, UPSTREAM_ACCEPT_ENCODING,
        UpstreamClientRegistry, decode_response_body,
    },
};

use super::{
    request_billing, request_billing_multiplier, request_billing_multiplier_for_value,
    request_body::{
        ImageBodySpoolSnapshot, ImageEditBodyError, ImageEditBodyPolicy, PreparedRequestBody,
        ProxyRequestBodyLimits,
    },
    usage::{ResponseErrorDetails, SseTerminalOutcome, UsageCollector},
};

/// Data-plane use case backed by a single immutable configuration snapshot per
/// request and a process-shared upstream client registry.
#[derive(Clone)]
pub struct ProxyService {
    runtime: Arc<RuntimeConfig>,
    upstream_clients: Arc<UpstreamClientRegistry>,
    max_request_body_bytes: usize,
    image_edit_body: ImageEditBodyPolicy,
    request_log_sink: Arc<dyn RequestLogSink>,
    routing: RoutingRuntime,
    admission: AdmissionRuntime,
    automatic_disable: Option<AutomaticDisableService>,
    connectors: UpstreamConnectorRegistry,
    websocket_lifecycle: websocket::WebSocketLifecycle,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WebSocketRuntimeSnapshot {
    pub(crate) active_downstream_sessions: u64,
    pub(crate) enabled: bool,
    pub(crate) idle_upstream_connections: u64,
    pub(crate) leased_upstream_connections: u64,
    pub(crate) pool_capacity: u64,
    pub(crate) pool_hits_total: u64,
    pub(crate) pool_misses_total: u64,
    pub(crate) pool_discarded_total: u64,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) max_connection_age_seconds: u64,
}

impl ProxyService {
    pub fn new<L>(
        runtime: Arc<RuntimeConfig>,
        request_body_limits: L,
    ) -> Result<Self, reqwest::Error>
    where
        L: Into<ProxyRequestBodyLimits>,
    {
        Self::with_log_sink(runtime, request_body_limits, Arc::new(NoopRequestLogSink))
    }

    pub fn with_log_sink<L>(
        runtime: Arc<RuntimeConfig>,
        request_body_limits: L,
        request_log_sink: Arc<dyn RequestLogSink>,
    ) -> Result<Self, reqwest::Error>
    where
        L: Into<ProxyRequestBodyLimits>,
    {
        Self::with_log_sink_and_routing(
            runtime,
            request_body_limits,
            request_log_sink,
            RoutingRuntime::new(crate::routing::PassiveHealthPolicy::default()),
        )
    }

    pub fn with_log_sink_and_routing<L>(
        runtime: Arc<RuntimeConfig>,
        request_body_limits: L,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
    ) -> Result<Self, reqwest::Error>
    where
        L: Into<ProxyRequestBodyLimits>,
    {
        Self::with_dependencies(
            runtime,
            request_body_limits,
            request_log_sink,
            routing,
            AdmissionRuntime::new(),
        )
    }

    pub fn with_dependencies<L>(
        runtime: Arc<RuntimeConfig>,
        request_body_limits: L,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
    ) -> Result<Self, reqwest::Error>
    where
        L: Into<ProxyRequestBodyLimits>,
    {
        Self::with_dependencies_and_registry(
            runtime,
            request_body_limits,
            Arc::new(UpstreamClientRegistry::new()),
            request_log_sink,
            routing,
            admission,
        )
    }

    /// Constructs a proxy with a process-shared registry supplied by the host.
    pub fn with_dependencies_and_registry<L>(
        runtime: Arc<RuntimeConfig>,
        request_body_limits: L,
        upstream_clients: Arc<UpstreamClientRegistry>,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
    ) -> Result<Self, reqwest::Error>
    where
        L: Into<ProxyRequestBodyLimits>,
    {
        Self::with_dependencies_and_registry_and_automation(
            runtime,
            request_body_limits,
            upstream_clients,
            request_log_sink,
            routing,
            admission,
            None,
        )
    }

    /// Adds asynchronous automatic-disable reporting to the proxy without
    /// allowing persistence to delay client-visible forwarding.
    pub fn with_dependencies_and_registry_and_automation<L>(
        runtime: Arc<RuntimeConfig>,
        request_body_limits: L,
        upstream_clients: Arc<UpstreamClientRegistry>,
        request_log_sink: Arc<dyn RequestLogSink>,
        routing: RoutingRuntime,
        admission: AdmissionRuntime,
        automatic_disable: Option<AutomaticDisableService>,
    ) -> Result<Self, reqwest::Error>
    where
        L: Into<ProxyRequestBodyLimits>,
    {
        let request_body_limits = request_body_limits.into();
        let snapshot = runtime.snapshot();
        routing.reconcile(&snapshot);
        upstream_clients.configure_websockets(snapshot.system_settings().websocket());
        Ok(Self {
            runtime,
            upstream_clients,
            max_request_body_bytes: request_body_limits.proxy_body_bytes,
            image_edit_body: request_body_limits.image_edit().clone(),
            request_log_sink,
            routing,
            admission,
            automatic_disable,
            connectors: UpstreamConnectorRegistry::default(),
            websocket_lifecycle: websocket::WebSocketLifecycle::new(),
        })
    }

    #[must_use]
    pub fn with_connector_registry(mut self, connectors: UpstreamConnectorRegistry) -> Self {
        self.connectors = connectors;
        self
    }

    pub(crate) async fn image_body_spool_snapshot(&self) -> ImageBodySpoolSnapshot {
        self.image_edit_body.spool_snapshot().await
    }

    pub async fn proxy(
        &self,
        api_operation: ApiOperation,
        request: Request<Body>,
    ) -> Result<AxumResponse, ProxyError> {
        let started_at = Instant::now();
        let started_wall_at = chrono::Utc::now();
        let (parts, body) = request.into_parts();
        let (snapshot, api_key) = self.authenticate_api_key(&parts.headers)?;
        self.proxy_authenticated_parts(
            api_operation,
            parts,
            body,
            snapshot,
            api_key,
            RequestLogSource::Client,
            started_wall_at,
            started_at,
        )
        .await
    }

    pub(crate) fn authenticate_api_key(
        &self,
        headers: &HeaderMap,
    ) -> Result<
        (
            Arc<crate::domain::CompiledRuntimeConfig>,
            Arc<CompiledApiKey>,
        ),
        ProxyError,
    > {
        let snapshot = self.runtime.snapshot();
        let api_key = self.authenticate_api_key_in_snapshot(headers, &snapshot)?;
        Ok((snapshot, api_key))
    }

    pub(crate) fn authenticate_api_key_in_snapshot(
        &self,
        headers: &HeaderMap,
        snapshot: &Arc<crate::domain::CompiledRuntimeConfig>,
    ) -> Result<Arc<CompiledApiKey>, ProxyError> {
        let client_key = parse_bearer_token(headers).inspect_err(|_| {
            trace_unlogged("invalid_api_key");
        })?;
        let api_key = snapshot.authenticate(client_key).ok_or_else(|| {
            trace_unlogged("invalid_or_expired_api_key");
            ProxyError::invalid_api_key()
        })?;
        Ok(api_key)
    }

    #[cfg(feature = "mcp-server")]
    pub(crate) async fn proxy_authenticated(
        &self,
        api_operation: ApiOperation,
        request: Request<Body>,
        snapshot: Arc<crate::domain::CompiledRuntimeConfig>,
        api_key: Arc<CompiledApiKey>,
        request_source: RequestLogSource,
    ) -> Result<AxumResponse, ProxyError> {
        let started_at = Instant::now();
        let started_wall_at = chrono::Utc::now();
        let (parts, body) = request.into_parts();
        self.proxy_authenticated_parts(
            api_operation,
            parts,
            body,
            snapshot,
            api_key,
            request_source,
            started_wall_at,
            started_at,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn proxy_authenticated_parts(
        &self,
        api_operation: ApiOperation,
        parts: axum::http::request::Parts,
        body: Body,
        snapshot: Arc<crate::domain::CompiledRuntimeConfig>,
        api_key: Arc<CompiledApiKey>,
        request_source: RequestLogSource,
        started_wall_at: chrono::DateTime<chrono::Utc>,
        started_at: Instant,
    ) -> Result<AxumResponse, ProxyError> {
        let api_format = api_operation.api_format();
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

        let original_body = if api_operation == ApiOperation::ImagesEdit {
            match self.image_edit_body.capture(&parts.headers, body).await {
                Ok(value) => PreparedRequestBody::ImageEdit(value),
                Err(error) => {
                    trace_unlogged("invalid_image_edit_body");
                    return Err(ProxyError::image_edit_body(error));
                }
            }
        } else {
            match to_bytes(body, self.max_request_body_bytes).await {
                Ok(value) => PreparedRequestBody::Json(value),
                Err(error) => {
                    trace_unlogged("unreadable_or_oversized_body");
                    return Err(request_body_error(error));
                }
            }
        };
        let parsed = match parse_request(api_operation, &original_body) {
            Ok(value) => value,
            Err(error) => {
                trace_unlogged("malformed_or_overlength_model");
                return Err(error);
            }
        };
        let interface = RequestInterface::for_http(api_operation);
        let (original_body, client_body_changed) =
            match original_body.apply_policy(RequestPolicyLayer::Client, interface) {
                Ok(value) => value,
                Err(error) => {
                    trace_unlogged("request_policy_rejected");
                    return Err(ProxyError::request_policy(error));
                }
            };
        let client_headers = match filter_client_headers(interface, &parts.headers) {
            Ok(headers) => headers,
            Err(error) => {
                trace_unlogged("request_policy_rejected");
                return Err(ProxyError::request_policy(error));
            }
        };
        let session_affinity = match_session_affinity(
            snapshot.system_settings().session_affinity(),
            api_format,
            &parsed.model,
            &client_headers,
            original_body
                .json_bytes()
                .map(Bytes::as_ref)
                .unwrap_or_default(),
        );
        let route = match self.routing.select_operation_with_affinity(
            &snapshot,
            &api_key,
            api_operation,
            &parsed.model,
            session_affinity.clone(),
        ) {
            SelectionResult::Selected(route) => route,
            SelectionResult::UnknownOrInaccessibleModel => {
                self.record_rejected(
                    &api_key,
                    request_source,
                    api_format,
                    api_operation,
                    &parsed.model,
                    &parsed.log_metadata,
                    parsed.request_protocol,
                    started_wall_at,
                    started_at,
                );
                return Err(ProxyError::unknown_model(&parsed.model));
            }
            SelectionResult::NoHealthyChannel { rule } => {
                self.record_no_healthy_channel(
                    &api_key,
                    request_source,
                    api_format,
                    api_operation,
                    &parsed.model,
                    &parsed.log_metadata,
                    parsed.request_protocol,
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
            channel_slot,
            session_affinity: selected_session_affinity,
            lease,
        } = route;
        let mut current_rule = rule;
        let mut current_channel = channel;
        let mut current_channel_slot = channel_slot;
        let mut current_session_affinity = selected_session_affinity;
        let request_multiplier =
            request_billing_multiplier_for_body(current_rule.advanced_billing(), &original_body);
        let mut completion = CompletionGuard::new(
            Arc::clone(&self.request_log_sink),
            &api_key,
            request_source,
            &parsed.model,
            &parsed.log_metadata,
            parsed.request_protocol,
            api_format,
            api_operation,
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
        let mut attempted_channel_slots = AttemptedChannelSlots::new();

        loop {
            attempted_channel_slots.push(current_channel_slot);
            let affinity_hit = current_session_affinity
                .as_ref()
                .is_some_and(SessionAffinitySelection::cache_hit);
            let prepared_attempt = match self.connectors.prepare(
                &current_channel,
                api_operation,
                affinity_hit,
                &client_headers,
                session_affinity
                    .as_ref()
                    .map(SessionAffinityMatch::session_hash),
                snapshot.system_settings().codex(),
            ) {
                Ok(attempt) => attempt,
                Err(error) => {
                    if affinity_hit {
                        completion.set_preserve_affinity_on_failure(true);
                        let error = ProxyError::sticky_connector_unavailable(error);
                        completion
                            .finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                        return Err(error);
                    }
                    let retry_route = self.routing.select_operation_with_affinity_excluding(
                        &snapshot,
                        &api_key,
                        api_operation,
                        &parsed.model,
                        session_affinity.clone(),
                        attempted_channel_slots.as_slice(),
                    );
                    let SelectionResult::Selected(route) = retry_route else {
                        let error = ProxyError::connector_unavailable(error);
                        completion
                            .finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                        return Err(error);
                    };
                    let crate::routing::SelectedRoute {
                        rule,
                        channel,
                        channel_slot,
                        session_affinity: selected_session_affinity,
                        lease,
                    } = route;
                    let request_billing_multiplier = request_billing_multiplier_for_body(
                        rule.advanced_billing(),
                        &original_body,
                    );
                    completion.replace_route_before_dispatch(
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
                    current_channel_slot = channel_slot;
                    current_session_affinity = selected_session_affinity;
                    continue;
                }
            };
            completion
                .set_preserve_affinity_on_failure(prepared_attempt.preserves_affinity_on_failure());
            let transforms = current_channel.upstream_policy().effective_transforms();
            if api_operation == ApiOperation::StandaloneWebSearch
                && !transforms.request_json().is_empty()
            {
                let error = ProxyError::invalid_request_with_code(
                    "Standalone web search does not support request JSON transforms.",
                    "body",
                    "standalone_web_search_json_transform_unsupported",
                );
                completion.finish_with_proxy_error(RequestOutcome::ClientRequestError, &error);
                return Err(error);
            }
            let model_rewritten = parsed.model != current_rule.upstream_model();
            let body = match original_body
                .clone()
                .rewrite_model(&parsed.model, current_rule.upstream_model())
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let error = ProxyError::image_edit_body(error);
                    completion.finish_with_proxy_error(RequestOutcome::ClientRequestError, &error);
                    return Err(error);
                }
            };
            let body = match apply_request_json_transform(body, transforms.request_json()) {
                Ok(body) => body,
                Err(error) => {
                    completion.finish_with_proxy_error(RequestOutcome::ClientRequestError, &error);
                    return Err(error);
                }
            };
            let body = match prepared_attempt
                .adapt_body(body, parsed.request_protocol)
                .await
            {
                Ok(body) => body,
                Err(error) => {
                    let error = ProxyError::connector_attempt(error);
                    completion.set_client_visible_status(error.status.as_u16());
                    let outcome = if error.status.is_client_error() {
                        RequestOutcome::ClientRequestError
                    } else {
                        RequestOutcome::UpstreamUnavailable
                    };
                    completion.finish_with_proxy_error(outcome, &error);
                    return Err(error);
                }
            };
            let request_body_changed = client_body_changed
                || model_rewritten
                || !transforms.request_json().is_empty()
                || prepared_attempt.changes_request_body();

            // Apply the plan before hop-by-hop cleanup so `HeaderPlan` can
            // reject dynamically protected names declared by the client
            // `Connection` header. Stale client entity metadata is removed
            // first so the plan may explicitly supply values for the final
            // body. Cleanup then removes hop-by-hop names again.
            let mut headers = client_headers.clone();
            if request_body_changed {
                remove_rewritten_request_entity_headers(&mut headers);
            }
            if let Err(apply_error) = apply_header_plan(&mut headers, transforms.request_headers())
            {
                let error = ProxyError::transform_failed();
                completion.finish_with_error_details(
                    RequestOutcome::ClientRequestError,
                    proxy_error_with_source(&error, &apply_error),
                );
                return Err(error);
            }
            let mut headers = forward_request_headers(&headers);

            let url = match prepared_attempt.upstream_url(&current_channel, &parts.uri) {
                Ok(value) => value,
                Err(error) => {
                    let error = ProxyError::connector_attempt(error);
                    completion.finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                    return Err(error);
                }
            };
            if let Err(error) = prepared_attempt.inject_headers(
                &mut headers,
                &current_channel,
                parsed.request_protocol,
            ) {
                let error = ProxyError::connector_attempt(error);
                completion.finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                return Err(error);
            }
            headers.insert(
                ACCEPT_ENCODING,
                if headers.contains_key(RANGE) {
                    HeaderValue::from_static("identity")
                } else {
                    HeaderValue::from_static(UPSTREAM_ACCEPT_ENCODING)
                },
            );
            strip_explicitly_ignored_client_headers(&mut headers);
            let upstream_policy = match ResolvedUpstreamPolicy::try_resolve_for_operation(
                api_operation,
                &snapshot.system_settings().upstream_timeouts(),
                current_channel.upstream_policy(),
            ) {
                Ok(policy) => policy,
                Err(_) => {
                    let error = ProxyError::upstream_unavailable();
                    completion.finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                    return Err(error);
                }
            };
            let upstream_client = match self
                .upstream_clients
                .client_for(current_channel.upstream_policy(), upstream_policy)
            {
                Ok(client) => client,
                Err(_) => {
                    let error = ProxyError::upstream_unavailable();
                    completion.finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                    return Err(error);
                }
            };
            headers.insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&body.len().to_string())
                    .expect("request body length is a valid header value"),
            );
            let body = match body.reqwest_body().await {
                Ok(body) => body,
                Err(error) => {
                    let error = ProxyError::image_edit_body(error);
                    completion.set_client_visible_status(error.status.as_u16());
                    completion.finish_with_proxy_error(RequestOutcome::UpstreamUnavailable, &error);
                    return Err(error);
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
                Ok(Err(source)) => {
                    let error = ProxyError::upstream_unavailable();
                    completion.finish_with_error_details(
                        RequestOutcome::UpstreamUnavailable,
                        proxy_error_with_source(&error, &source),
                    );
                    return Err(error);
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
                    let retry_route = (prepared_attempt.allows_automatic_retry()
                        && api_operation.permits_automatic_retry()
                        && attempt < max_attempts)
                        .then(|| {
                            self.routing.select_operation_with_affinity_excluding(
                                &snapshot,
                                &api_key,
                                api_operation,
                                &parsed.model,
                                session_affinity.clone(),
                                attempted_channel_slots.as_slice(),
                            )
                        });
                    let Some(SelectionResult::Selected(route)) = retry_route else {
                        let error = failure.proxy_error();
                        completion.finish_with_proxy_error(failure.outcome(), &error);
                        return Err(error);
                    };
                    let crate::routing::SelectedRoute {
                        rule,
                        channel,
                        channel_slot,
                        session_affinity: selected_session_affinity,
                        lease,
                    } = route;
                    let failed_channel_id = current_channel.id();
                    let next_channel_id = channel.id();
                    let request_billing_multiplier = request_billing_multiplier_for_body(
                        rule.advanced_billing(),
                        &original_body,
                    );
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
                    current_channel_slot = channel_slot;
                    current_session_affinity = selected_session_affinity;
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

            let connector_success_response_is_sse = prepared_attempt.successful_response_is_sse();
            prepared_attempt.observe_response(upstream_response.status());

            return response_from_upstream(
                upstream_response,
                upstream_policy.timeouts().stream_idle(),
                completion,
                connector_success_response_is_sse,
                transforms.response_headers(),
                transforms.sse_event_patches().clone(),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_rejected(
        &self,
        api_key: &CompiledApiKey,
        request_source: RequestLogSource,
        api_format: ApiFormat,
        api_operation: ApiOperation,
        client_model: &str,
        log_metadata: &RequestLogMetadata,
        request_protocol: RequestProtocol,
        started_at: chrono::DateTime<chrono::Utc>,
        started: Instant,
    ) {
        let elapsed = clamp_duration_ms(started.elapsed());
        let error = ProxyError::unknown_model(client_model);
        let event = RequestLogEvent {
            id: Uuid::new_v4(),
            started_at,
            completed_at: completed_at(started_at, started.elapsed()),
            user_id: api_key.user_id(),
            api_key_id: api_key.id(),
            request_source,
            api_format,
            api_operation,
            request_protocol,
            client_model: client_model.to_owned(),
            reasoning_effort: log_metadata.reasoning_effort.clone(),
            fast_mode: log_metadata.fast_mode,
            upstream_model: None,
            model_rule_id: None,
            channel_group_id: None,
            channel_id: None,
            model_id: None,
            outcome: RequestLogOutcome::Rejected,
            response_status_code: Some(StatusCode::NOT_FOUND.as_u16()),
            streamed: request_protocol.is_streamed(),
            ttft_ms: None,
            total_duration_ms: elapsed,
            billing: None,
            error_code: error.code.map(str::to_owned),
            error_summary: error.request_log_details().summary,
        };
        tracing::info!(event = "proxy_request_completed", api_key_id = %api_key.id(), api_format = ?api_format, outcome = "rejected", "proxy request completed");
        self.request_log_sink.try_record(event);
    }

    #[allow(clippy::too_many_arguments)]
    fn record_no_healthy_channel(
        &self,
        api_key: &CompiledApiKey,
        request_source: RequestLogSource,
        api_format: ApiFormat,
        api_operation: ApiOperation,
        client_model: &str,
        log_metadata: &RequestLogMetadata,
        request_protocol: RequestProtocol,
        rule: &CompiledModelRule,
        started_at: chrono::DateTime<chrono::Utc>,
        started: Instant,
    ) {
        let error = ProxyError::no_healthy_channel();
        let event = RequestLogEvent {
            id: Uuid::new_v4(),
            started_at,
            completed_at: completed_at(started_at, started.elapsed()),
            user_id: api_key.user_id(),
            api_key_id: api_key.id(),
            request_source,
            api_format,
            api_operation,
            request_protocol,
            client_model: client_model.to_owned(),
            reasoning_effort: log_metadata.reasoning_effort.clone(),
            fast_mode: log_metadata.fast_mode,
            upstream_model: Some(rule.upstream_model().to_owned()),
            model_rule_id: Some(rule.id()),
            channel_group_id: None,
            channel_id: None,
            model_id: Some(rule.upstream_model_id()),
            outcome: RequestLogOutcome::Failed,
            response_status_code: Some(StatusCode::SERVICE_UNAVAILABLE.as_u16()),
            streamed: request_protocol.is_streamed(),
            ttft_ms: None,
            total_duration_ms: clamp_duration_ms(started.elapsed()),
            billing: None,
            error_code: error.code.map(str::to_owned),
            error_summary: error.request_log_details().summary,
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

        let can_list_models = ApiFormat::ALL.into_iter().any(|api_format| {
            api_key.permits(api_format, ApiKeyPermission::Proxy)
                && api_key.permits(api_format, ApiKeyPermission::ModelsRead)
        });
        if !can_list_models {
            return Err(ProxyError::forbidden(
                "This API key cannot list models in any API format.",
            ));
        }

        let models = ApiFormat::ALL
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
    #[cfg(feature = "mcp-server")]
    #[must_use]
    pub(crate) const fn status(&self) -> StatusCode {
        self.status
    }

    #[cfg(feature = "mcp-server")]
    #[must_use]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

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

    fn websocket_disabled(message: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.to_owned(),
            error_type: "permission_error",
            param: None,
            code: Some("websocket_disabled"),
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

    fn image_edit_body(error: ImageEditBodyError) -> Self {
        let (status, message, param, code) = match error {
            ImageEditBodyError::RequestPolicy(error) => return Self::request_policy(error),
            ImageEditBodyError::BodyTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "Images edit request body exceeds the configured size limit.",
                "body",
                "request_too_large",
            ),
            ImageEditBodyError::FileTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "An Images edit file exceeds the configured size limit.",
                "image",
                "image_edit_file_too_large",
            ),
            ImageEditBodyError::TextFieldTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "Images edit text fields exceed the supported size limits.",
                "body",
                "image_edit_field_too_large",
            ),
            ImageEditBodyError::UnsupportedContentType => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Images edits require multipart/form-data with a valid boundary.",
                "content_type",
                "image_edit_content_type_unsupported",
            ),
            ImageEditBodyError::UnsupportedContentEncoding => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Images edits do not support encoded request bodies.",
                "content_encoding",
                "image_edit_content_encoding_unsupported",
            ),
            ImageEditBodyError::TooManyFields => (
                StatusCode::BAD_REQUEST,
                "Images edit multipart body contains too many fields.",
                "body",
                "image_edit_too_many_fields",
            ),
            ImageEditBodyError::FieldNameTooLong => (
                StatusCode::BAD_REQUEST,
                "Images edit multipart field name is too long.",
                "body",
                "image_edit_field_name_too_long",
            ),
            ImageEditBodyError::FileNameTooLong => (
                StatusCode::BAD_REQUEST,
                "Images edit multipart file name is too long.",
                "image",
                "image_edit_file_name_too_long",
            ),
            ImageEditBodyError::UnexpectedFileField => (
                StatusCode::BAD_REQUEST,
                "Images edit file fields must be named image, image[], or mask.",
                "body",
                "image_edit_file_field_unsupported",
            ),
            ImageEditBodyError::TooManyImages => (
                StatusCode::BAD_REQUEST,
                "Images edits support at most 16 input images.",
                "image",
                "image_edit_too_many_images",
            ),
            ImageEditBodyError::TooManyMasks => (
                StatusCode::BAD_REQUEST,
                "Images edits support at most one mask.",
                "mask",
                "image_edit_too_many_masks",
            ),
            ImageEditBodyError::MissingImage => (
                StatusCode::BAD_REQUEST,
                "Images edit request must contain at least one image.",
                "image",
                "image_edit_image_required",
            ),
            ImageEditBodyError::MissingModel
            | ImageEditBodyError::InvalidModel
            | ImageEditBodyError::InvalidJson => (
                StatusCode::BAD_REQUEST,
                "Request body must contain a string model.",
                "model",
                "invalid_request",
            ),
            ImageEditBodyError::DuplicateModel => (
                StatusCode::BAD_REQUEST,
                "Images edit request must contain exactly one model field.",
                "model",
                "image_edit_duplicate_model",
            ),
            ImageEditBodyError::EmptyModel => (
                StatusCode::BAD_REQUEST,
                "Request body must contain a non-empty model.",
                "model",
                "invalid_request",
            ),
            ImageEditBodyError::ModelTooLong => (
                StatusCode::BAD_REQUEST,
                "Request model exceeds the supported length.",
                "model",
                "invalid_request",
            ),
            ImageEditBodyError::StreamingUnsupported => (
                StatusCode::BAD_REQUEST,
                "Image API streaming is not supported yet.",
                "stream",
                "image_streaming_unsupported",
            ),
            ImageEditBodyError::JsonTransformUnsupported => (
                StatusCode::BAD_REQUEST,
                "Images edit multipart bodies do not support request JSON transforms.",
                "body",
                "image_edit_json_transform_unsupported",
            ),
            ImageEditBodyError::CodexTooManyImages => (
                StatusCode::BAD_REQUEST,
                "Codex OAuth Images edits support at most five input images.",
                "image",
                "codex_image_edit_too_many_images",
            ),
            ImageEditBodyError::CodexMissingField => (
                StatusCode::BAD_REQUEST,
                "Codex OAuth Images edit request is missing a required field.",
                "body",
                "codex_image_edit_field_required",
            ),
            ImageEditBodyError::CodexDuplicateField => (
                StatusCode::BAD_REQUEST,
                "Codex OAuth Images edit request contains a duplicate field.",
                "body",
                "codex_image_edit_duplicate_field",
            ),
            ImageEditBodyError::CodexInvalidField => (
                StatusCode::BAD_REQUEST,
                "Codex OAuth Images edit request contains an invalid field value.",
                "body",
                "codex_image_edit_field_invalid",
            ),
            ImageEditBodyError::CodexImageContentType => (
                StatusCode::BAD_REQUEST,
                "Codex OAuth Images edit inputs require a recognized image content type.",
                "image",
                "codex_image_edit_content_type_required",
            ),
            ImageEditBodyError::Unreadable | ImageEditBodyError::MalformedMultipart => (
                StatusCode::BAD_REQUEST,
                "Images edit multipart body could not be read.",
                "body",
                "image_edit_multipart_invalid",
            ),
            ImageEditBodyError::StorageUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Images edit request storage is temporarily unavailable.",
                "body",
                "image_body_spool_unavailable",
            ),
        };
        Self {
            status,
            message: message.to_owned(),
            error_type: if status.is_server_error() {
                "api_error"
            } else {
                "invalid_request_error"
            },
            param: Some(param),
            code: Some(code),
            authenticate: false,
            retry_after: None,
        }
    }

    fn invalid_request(message: &'static str, param: &'static str) -> Self {
        Self::invalid_request_with_code(message, param, "invalid_request")
    }

    fn invalid_request_with_code(
        message: &'static str,
        param: &'static str,
        code: &'static str,
    ) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.to_owned(),
            error_type: "invalid_request_error",
            param: Some(param),
            code: Some(code),
            authenticate: false,
            retry_after: None,
        }
    }

    fn request_policy(error: RequestPolicyError) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.message(),
            error_type: "invalid_request_error",
            param: Some(error.param()),
            code: Some(error.code()),
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

    fn unsupported_upstream_content_encoding() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "The upstream response used an unsupported content encoding.".to_owned(),
            error_type: "api_error",
            param: None,
            code: Some("upstream_content_encoding_unsupported"),
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

    fn connector_attempt(error: ConnectorAttemptError) -> Self {
        match error {
            ConnectorAttemptError::ClientRequest {
                message,
                param,
                code,
            } => Self {
                status: StatusCode::BAD_REQUEST,
                message: message.to_owned(),
                error_type: "invalid_request_error",
                param: Some(param),
                code: Some(code),
                authenticate: false,
                retry_after: None,
            },
            ConnectorAttemptError::RequestBody(error) => Self::image_edit_body(error),
            ConnectorAttemptError::RequestEncoding => Self {
                status: StatusCode::BAD_GATEWAY,
                message: "The selected upstream connector could not encode the request body."
                    .to_owned(),
                error_type: "api_error",
                param: None,
                code: Some("upstream_request_encoding_failed"),
                authenticate: false,
                retry_after: None,
            },
            ConnectorAttemptError::RequestPolicy(error) => Self::request_policy(error),
            ConnectorAttemptError::InvalidTarget => Self {
                status: StatusCode::BAD_GATEWAY,
                message: "The selected upstream channel has an invalid target URL.".to_owned(),
                error_type: "api_error",
                param: None,
                code: Some("invalid_upstream_url"),
                authenticate: false,
                retry_after: None,
            },
            ConnectorAttemptError::InvalidCredentials => Self {
                status: StatusCode::BAD_GATEWAY,
                message: "The selected upstream connector has invalid credentials.".to_owned(),
                error_type: "api_error",
                param: None,
                code: Some("invalid_upstream_credentials"),
                authenticate: false,
                retry_after: None,
            },
        }
    }

    fn connector_unavailable(reason: ConnectorUnavailable) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "The selected upstream connector is currently unavailable.".to_owned(),
            error_type: "api_error",
            param: None,
            code: Some(reason.code()),
            authenticate: false,
            retry_after: None,
        }
    }

    fn sticky_connector_unavailable(reason: ConnectorUnavailable) -> Self {
        let mut error = Self::connector_unavailable(reason);
        error.message =
            "The upstream connector pinned to this session is currently unavailable.".to_owned();
        error.code = Some(reason.sticky_code());
        error
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

    fn shutting_down() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "The gateway is shutting down and cannot accept a new WebSocket.".to_owned(),
            error_type: "api_error",
            param: None,
            code: Some("server_shutting_down"),
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

    fn request_log_details(&self) -> ResponseErrorDetails {
        ResponseErrorDetails::from_json(&json!({
            "error": {
                "message": self.message,
                "type": self.error_type,
                "param": self.param,
                "code": self.code,
            }
        }))
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
        if let Some(retry_after) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&retry_after.to_string())
        {
            response.headers_mut().insert("retry-after", value);
        }
        response
    }
}

fn proxy_error_with_source(error: &ProxyError, source: &dyn Error) -> ResponseErrorDetails {
    let base = error
        .request_log_details()
        .summary
        .unwrap_or_else(|| error.message.clone());
    ResponseErrorDetails::from_message(
        error.code,
        &format!(
            "{base}\n\nSource error chain:\n{}",
            format_error_chain(source)
        ),
    )
}

fn format_error_chain(error: &dyn Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        rendered.push_str("\nCaused by: ");
        rendered.push_str(&next.to_string());
        source = next.source();
    }
    rendered
}

#[derive(Deserialize)]
struct RequestProbe {
    model: String,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    reasoning_effort: Value,
    #[serde(default)]
    reasoning: Value,
    #[serde(default)]
    service_tier: Value,
}

struct ParsedRequest {
    model: String,
    log_metadata: RequestLogMetadata,
    request_protocol: RequestProtocol,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RequestLogMetadata {
    reasoning_effort: Option<String>,
    fast_mode: bool,
}

fn parse_request(
    api_operation: ApiOperation,
    body: &PreparedRequestBody,
) -> Result<ParsedRequest, ProxyError> {
    let api_format = api_operation.api_format();
    if let Some(edit) = body.image_edit() {
        if edit.stream_requested() {
            return Err(ProxyError::image_edit_body(
                ImageEditBodyError::StreamingUnsupported,
            ));
        }
        return Ok(ParsedRequest {
            model: edit.model().to_owned(),
            log_metadata: RequestLogMetadata::default(),
            request_protocol: RequestProtocol::NonStream,
        });
    }
    let body = body
        .json_bytes()
        .expect("non-edit requests use validated JSON bodies");
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
    if api_operation.is_images() && probe.stream {
        return Err(ProxyError::invalid_request_with_code(
            "Image API streaming is not supported yet.",
            "stream",
            "image_streaming_unsupported",
        ));
    }
    Ok(ParsedRequest {
        model: probe.model,
        log_metadata: request_log_metadata(
            api_format,
            &probe.reasoning_effort,
            &probe.reasoning,
            &probe.service_tier,
        ),
        request_protocol: RequestProtocol::from_http_streamed(probe.stream),
    })
}

fn request_billing_multiplier_for_body(
    advanced_billing: &CompiledAdvancedBilling,
    body: &PreparedRequestBody,
) -> Decimal {
    if !advanced_billing.has_request_multipliers() {
        return Decimal::ONE;
    }
    match body.json_bytes() {
        Some(body) => request_billing_multiplier(advanced_billing, body),
        None => request_billing_multiplier_for_value(advanced_billing, &body.request_value()),
    }
}

fn apply_request_json_transform(
    body: PreparedRequestBody,
    plan: &JsonPatchPlan,
) -> Result<PreparedRequestBody, ProxyError> {
    match body {
        PreparedRequestBody::Json(body) => apply_json_patch_plan(body, plan)
            .map(PreparedRequestBody::Json)
            .map_err(|_| ProxyError::transform_failed()),
        PreparedRequestBody::ImageEdit(body) if plan.is_empty() => {
            Ok(PreparedRequestBody::ImageEdit(body))
        }
        PreparedRequestBody::ImageEdit(_) => Err(ProxyError::image_edit_body(
            ImageEditBodyError::JsonTransformUnsupported,
        )),
    }
}

fn request_log_metadata(
    api_format: ApiFormat,
    reasoning_effort: &Value,
    reasoning: &Value,
    service_tier: &Value,
) -> RequestLogMetadata {
    let top_level_effort = normalized_request_label(reasoning_effort);
    let nested_effort = reasoning.get("effort").and_then(normalized_request_label);
    let reasoning_effort = match api_format {
        ApiFormat::OpenAiChatCompletions => top_level_effort.or(nested_effort),
        ApiFormat::OpenAiResponses => nested_effort.or(top_level_effort),
        ApiFormat::OpenAiImages => None,
    };
    RequestLogMetadata {
        reasoning_effort,
        fast_mode: api_format != ApiFormat::OpenAiImages
            && normalized_request_label(service_tier).as_deref() == Some("priority"),
    }
}

fn normalized_request_label(value: &Value) -> Option<String> {
    const MAX_LABEL_CHARS: usize = 32;

    let value = value.as_str()?.trim();
    (!value.is_empty()
        && value.chars().count() <= MAX_LABEL_CHARS
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')))
    .then(|| value.to_ascii_lowercase())
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

fn forward_request_headers(headers: &HeaderMap) -> HeaderMap {
    forward_headers(headers, true)
}

fn forward_response_headers(headers: &HeaderMap) -> HeaderMap {
    forward_headers(headers, false)
}

fn remove_rewritten_request_entity_headers(headers: &mut HeaderMap) {
    for name in [
        HeaderName::from_static("content-md5"),
        HeaderName::from_static("digest"),
        HeaderName::from_static("content-digest"),
        HeaderName::from_static("repr-digest"),
        HeaderName::from_static("etag"),
        HeaderName::from_static("last-modified"),
    ] {
        headers.remove(name);
    }
}

fn forward_headers(headers: &HeaderMap, request: bool) -> HeaderMap {
    let connection_names = connection_header_names(headers);
    let mut forwarded = HeaderMap::new();
    for (name, value) in headers {
        if is_hop_by_hop(name, &connection_names)
            || (request
                && (matches!(
                    *name,
                    HOST | CONTENT_LENGTH | AUTHORIZATION | PROXY_AUTHORIZATION | ACCEPT_ENCODING
                ) || client_header_explicitly_ignored(name)))
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
    connector_success_response_is_sse: bool,
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
    let content_codings = match ResponseContentCodings::parse(original_upstream_headers) {
        Ok(codings) => codings,
        Err(_) => {
            let error = ProxyError::unsupported_upstream_content_encoding();
            completion.set_client_visible_status(StatusCode::BAD_GATEWAY.as_u16());
            completion.finish_with_proxy_error(
                RequestOutcome::UpstreamContentEncodingUnsupported,
                &error,
            );
            return Err(error);
        }
    };
    let response_is_sse = is_sse_response(original_upstream_headers)
        || (connector_success_response_is_sse && upstream_status.is_success());
    let transform_sse = sse_event_patches.has_operations() && response_is_sse;
    let capture_error_body = !response_is_sse
        && !upstream_status.is_success()
        && response_error_body_is_textual(original_upstream_headers);
    completion.configure_usage_collector(response_is_sse, capture_error_body);
    let mut upstream_headers = original_upstream_headers.clone();
    if let Err(apply_error) = apply_response_header_plan(&mut upstream_headers, response_headers) {
        let error = ProxyError::response_transform_failed();
        completion.set_client_visible_status(StatusCode::BAD_GATEWAY.as_u16());
        completion.finish_with_error_details(
            RequestOutcome::ResponseTransformFailed,
            proxy_error_with_source(&error, &apply_error),
        );
        return Err(error);
    }
    if connector_success_response_is_sse && upstream_status.is_success() {
        upstream_headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    }
    upstream_headers.remove(CONTENT_ENCODING);
    if content_codings.is_encoded() {
        remove_decoded_entity_headers(&mut upstream_headers);
    }
    let mut headers = forward_response_headers(&upstream_headers);
    if transform_sse {
        remove_transformed_entity_headers(&mut headers);
    }
    let expected_body_bytes = (!transform_sse && !content_codings.is_encoded())
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
        decode_response_body(upstream_response, &content_codings),
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

fn response_error_body_is_textual(headers: &HeaderMap) -> bool {
    let Some(media_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let media_type = media_type.to_ascii_lowercase();
    !media_type.starts_with("image/")
        && !media_type.starts_with("audio/")
        && !media_type.starts_with("video/")
        && !media_type.starts_with("font/")
        && !media_type.starts_with("multipart/")
        && !matches!(
            media_type.as_str(),
            "application/octet-stream" | "application/pdf" | "application/zip" | "application/gzip"
        )
}

fn remove_decoded_entity_headers(headers: &mut HeaderMap) {
    for name in [
        CONTENT_ENCODING,
        CONTENT_LENGTH,
        ACCEPT_RANGES,
        HeaderName::from_static("etag"),
        HeaderName::from_static("content-md5"),
        HeaderName::from_static("digest"),
        HeaderName::from_static("content-digest"),
        HeaderName::from_static("repr-digest"),
    ] {
        headers.remove(name);
    }
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

type UpstreamByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, DecodedBodyError>> + Send>>;
type BodyStreamError = DecodedBodyError;

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
    upstream: impl Stream<Item = Result<Bytes, DecodedBodyError>> + Send + 'static,
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
                        Err(source) => {
                            state.upstream.take();
                            state.sse_transformer = None;
                            let error = ProxyError::response_transform_failed();
                            state.completion.finish_with_error_details(
                                RequestOutcome::ResponseTransformFailed,
                                proxy_error_with_source(&error, &source),
                            );
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
                        state.completion.finish_with_message(
                            RequestOutcome::UpstreamBodyError,
                            Some("upstream_body_error"),
                            &format_error_chain(error.as_ref()),
                        );
                        return Some((Err(error), state));
                    }
                    Ok(None) => {
                        state.upstream.take();
                        let default_outcome = completed_transport_outcome(state.upstream_succeeded);
                        if let Some(transformer) = &mut state.sse_transformer
                            && let Some(residual) = transformer.finish()
                        {
                            record_stream_bytes(&mut state, &residual);
                            let outcome = state
                                .completion
                                .finalize_usage()
                                .map(|terminal| {
                                    sse_terminal_request_outcome(terminal, state.upstream_succeeded)
                                })
                                .unwrap_or(default_outcome);
                            state.completion.finish(outcome);
                            return Some((Ok(residual), state));
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
                        state.completion.finish_with_message(
                            RequestOutcome::StreamIdleTimeout,
                            Some("stream_idle_timeout"),
                            "The upstream response stream was idle for too long.",
                        );
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
    .fuse()
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

const INLINE_ATTEMPTED_CHANNELS: usize = MAX_REQUEST_RETRIES as usize + 1;

struct AttemptedChannelSlots {
    inline: [usize; INLINE_ATTEMPTED_CHANNELS],
    len: usize,
    overflow: Option<Vec<usize>>,
}

impl AttemptedChannelSlots {
    const fn new() -> Self {
        Self {
            inline: [usize::MAX; INLINE_ATTEMPTED_CHANNELS],
            len: 0,
            overflow: None,
        }
    }

    fn push(&mut self, channel_slot: usize) {
        if let Some(slots) = &mut self.overflow {
            slots.push(channel_slot);
            return;
        }
        if self.len < self.inline.len() {
            self.inline[self.len] = channel_slot;
            self.len += 1;
            return;
        }
        let mut slots = Vec::with_capacity(self.inline.len().saturating_mul(2));
        slots.extend_from_slice(&self.inline);
        slots.push(channel_slot);
        self.overflow = Some(slots);
    }

    fn as_slice(&self) -> &[usize] {
        self.overflow.as_deref().unwrap_or(&self.inline[..self.len])
    }
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
    UpstreamContentEncodingUnsupported,
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
            Self::UpstreamContentEncodingUnsupported => "upstream_content_encoding_unsupported",
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
            | Self::UpstreamContentEncodingUnsupported
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
            Self::UpstreamContentEncodingUnsupported => {
                Some("upstream_content_encoding_unsupported")
            }
            Self::UpstreamBodyError => Some("upstream_body_error"),
            Self::StreamIdleTimeout => Some("stream_idle_timeout"),
            Self::ResponseTransformFailed => Some("response_transform_failed"),
            Self::Cancelled => Some("client_cancelled"),
            Self::ClientRequestError => Some("invalid_request"),
        }
    }

    fn default_error_summary(self, upstream_status: Option<u16>) -> Option<String> {
        let summary = match self {
            Self::Succeeded => return None,
            Self::UpstreamHttpError => {
                return Some(upstream_status.map_or_else(
                    || "The upstream returned an unsuccessful HTTP response.".to_owned(),
                    |status| format!("The upstream returned HTTP {status}."),
                ));
            }
            Self::UpstreamSseError => "The upstream stream reported an application-level error.",
            Self::ConnectTimeout => "Connecting to the selected upstream channel timed out.",
            Self::ResponseHeaderTimeout => {
                "The selected upstream channel did not return response headers in time."
            }
            Self::UpstreamUnavailable => "The selected upstream channel could not be reached.",
            Self::UpstreamContentEncodingUnsupported => {
                "The upstream response used an unsupported content encoding."
            }
            Self::UpstreamBodyError => {
                "The upstream response body ended with a transport or protocol error."
            }
            Self::StreamIdleTimeout => "The upstream response stream was idle for too long.",
            Self::ResponseTransformFailed => {
                "The upstream response transform could not be applied."
            }
            Self::Cancelled => "The downstream client disconnected before the request completed.",
            Self::ClientRequestError => {
                "The request could not be prepared for the selected upstream channel."
            }
        };
        Some(summary.to_owned())
    }

    const fn fallback_status(self) -> Option<u16> {
        match self {
            Self::ConnectTimeout | Self::ResponseHeaderTimeout => {
                Some(StatusCode::GATEWAY_TIMEOUT.as_u16())
            }
            Self::UpstreamUnavailable => Some(StatusCode::BAD_GATEWAY.as_u16()),
            Self::UpstreamContentEncodingUnsupported => Some(StatusCode::BAD_GATEWAY.as_u16()),
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
                | Self::UpstreamContentEncodingUnsupported
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
    reasoning_effort: Option<String>,
    fast_mode: bool,
    upstream_model: String,
    model_rule_id: Uuid,
    channel_group_id: Uuid,
    channel_id: Uuid,
    model_id: Uuid,
    api_format: ApiFormat,
    api_operation: ApiOperation,
    request_source: RequestLogSource,
    request_protocol: RequestProtocol,
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
    preserve_affinity_on_failure: bool,
    attempts: u32,
    error_details: Option<ResponseErrorDetails>,
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
        request_source: RequestLogSource,
        client_model: &str,
        log_metadata: &RequestLogMetadata,
        request_protocol: RequestProtocol,
        api_format: ApiFormat,
        api_operation: ApiOperation,
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
                reasoning_effort: log_metadata.reasoning_effort.clone(),
                fast_mode: log_metadata.fast_mode,
                upstream_model: rule.upstream_model().to_owned(),
                model_rule_id: rule.id(),
                channel_group_id: channel.group_id(),
                channel_id: channel.id(),
                model_id: rule.upstream_model_id(),
                api_format,
                api_operation,
                request_source,
                request_protocol,
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
                preserve_affinity_on_failure: false,
                attempts: 1,
                error_details: None,
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
            context.preserve_affinity_on_failure = false;
            context.attempts = context.attempts.saturating_add(1);
            context.error_details = None;
        }
        self.lease = Some(lease);
        self.automatic_disable = automatic_disable_context(
            channel,
            automatic_disable_service,
            automatic_disable_settings,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_route_before_dispatch(
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
            context.preserve_affinity_on_failure = false;
            context.error_details = None;
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

    fn set_preserve_affinity_on_failure(&mut self, preserve: bool) {
        if let Some(context) = &mut self.context {
            context.preserve_affinity_on_failure = preserve;
        }
    }

    fn configure_usage_collector(&mut self, sse: bool, capture_error_body: bool) {
        if let Some(context) = &mut self.context {
            context.usage = UsageCollector::new(context.api_format, sse);
            if capture_error_body {
                context.usage.capture_error_body();
            }
        }
    }

    fn set_error_details(&mut self, details: ResponseErrorDetails) {
        if let Some(context) = &mut self.context {
            context.error_details = Some(details);
        }
    }

    fn finish_with_error_details(
        &mut self,
        outcome: RequestOutcome,
        details: ResponseErrorDetails,
    ) {
        self.set_error_details(details);
        self.finish(outcome);
    }

    fn finish_with_proxy_error(&mut self, outcome: RequestOutcome, error: &ProxyError) {
        self.finish_with_error_details(outcome, error.request_log_details());
    }

    fn finish_with_message(&mut self, outcome: RequestOutcome, code: Option<&str>, message: &str) {
        self.finish_with_error_details(outcome, ResponseErrorDetails::from_message(code, message));
    }

    fn observe_usage(&mut self, bytes: &Bytes) -> Option<SseTerminalOutcome> {
        if let Some(context) = &mut self.context {
            context.usage.observe(bytes);
            context.usage.sse_terminal_outcome()
        } else {
            None
        }
    }

    fn observe_websocket_usage(&mut self, bytes: &Bytes) -> Option<SseTerminalOutcome> {
        self.context
            .as_mut()
            .and_then(|context| context.usage.observe_websocket_event(bytes))
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
            } else if outcome.evicts_session_affinity() && !context.preserve_affinity_on_failure {
                lease.request_failed();
            }
        }
        let usage = context.usage.latest();
        let upstream_error = context.usage.error_details().unwrap_or_default();
        let explicit_error = context.error_details.unwrap_or_default();
        let error_summary = if context.request_source == RequestLogSource::Mcp {
            outcome.default_error_summary(context.upstream_status)
        } else {
            upstream_error
                .summary
                .or(explicit_error.summary)
                .or_else(|| outcome.default_error_summary(context.upstream_status))
        };
        let error_code = if context.request_source == RequestLogSource::Mcp {
            outcome.error_code().map(str::to_owned)
        } else {
            upstream_error
                .code
                .or_else(|| outcome.error_code().map(str::to_owned))
        };
        let total_duration_ms = clamp_duration_ms(context.started_at.elapsed());
        let billing_ttft_ms = (!context.api_operation.is_images())
            .then(|| context.first_byte_at.map(clamp_duration_ms))
            .flatten();
        let billing = request_billing(
            &context.price_snapshot,
            &context.advanced_billing,
            context.billing_multiplier,
            context.request_billing_multiplier,
            usage,
            total_duration_ms,
            billing_ttft_ms,
        );
        tracing::info!(
            event = "proxy_request_completed",
            api_key_id = %context.api_key_id,
            client_model = %context.client_model,
            upstream_model = %context.upstream_model,
            channel_id = %context.channel_id,
            api_format = ?context.api_format,
            api_operation = context.api_operation.as_str(),
            request_protocol = context.request_protocol.as_str(),
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
            request_source: context.request_source,
            api_format: context.api_format,
            api_operation: context.api_operation,
            request_protocol: context.request_protocol,
            client_model: context.client_model,
            reasoning_effort: context.reasoning_effort,
            fast_mode: context.fast_mode,
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
            streamed: context.request_protocol.is_streamed(),
            ttft_ms: context.first_byte_at.map(clamp_duration_ms),
            total_duration_ms,
            billing: Some(billing),
            error_code,
            error_summary,
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
    use axum::{
        body::Bytes,
        http::{
            HeaderMap, HeaderValue, StatusCode,
            header::{ACCEPT_ENCODING, CONNECTION},
        },
    };
    use std::{sync::Arc, time::Duration};

    use regex::Regex;
    use reqwest::header::HeaderName;
    use rust_decimal::Decimal;

    use super::{
        AttemptedChannelSlots, PreparedRequestBody, forward_request_headers,
        forward_response_headers, match_session_affinity, parse_bearer_token, parse_request,
        response_error_body_is_textual, response_has_no_body,
    };
    use crate::{
        application::billing::{calculate_cost, request_billing},
        application::usage::ResponseUsage,
        domain::{
            AdvancedBilling, ApiFormat, ApiOperation, CompiledAdvancedBilling, LongContextTier,
            ModelPriceSnapshot, RequestBillingMultiplier, RequestPriceSnapshot, RequestUsage,
            SessionAffinityKeySource, SessionAffinityRule, SessionAffinitySettings,
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
    fn parses_deepseek_reasoning_effort_and_openai_fast_mode_from_chat_requests() {
        let parsed = parse_request(
            ApiOperation::ChatCompletions,
            &PreparedRequestBody::Json(Bytes::from_static(
                br#"{
                "model":"deepseek-v4",
                "reasoning_effort":"MAX",
                "service_tier":"priority"
            }"#,
            )),
        )
        .unwrap();

        assert_eq!(parsed.log_metadata.reasoning_effort.as_deref(), Some("max"));
        assert!(parsed.log_metadata.fast_mode);
    }

    #[test]
    fn parses_openai_nested_reasoning_effort_before_the_compatible_fallback() {
        let parsed = parse_request(
            ApiOperation::Responses,
            &PreparedRequestBody::Json(Bytes::from_static(
                br#"{
                "model":"gpt-5",
                "reasoning":{"effort":"xhigh"},
                "reasoning_effort":"low",
                "service_tier":"default"
            }"#,
            )),
        )
        .unwrap();

        assert_eq!(
            parsed.log_metadata.reasoning_effort.as_deref(),
            Some("xhigh")
        );
        assert!(!parsed.log_metadata.fast_mode);
    }

    #[test]
    fn ignores_unbounded_or_non_string_request_mode_metadata_without_rejecting() {
        let parsed = parse_request(
            ApiOperation::Responses,
            &PreparedRequestBody::Json(Bytes::from_static(
                br#"{
                "model":"gpt-5",
                "reasoning":{"effort":{"unexpected":true}},
                "reasoning_effort":"this-label-is-longer-than-thirty-two-characters",
                "service_tier":42
            }"#,
            )),
        )
        .unwrap();

        assert_eq!(parsed.log_metadata.reasoning_effort, None);
        assert!(!parsed.log_metadata.fast_mode);
    }

    #[test]
    fn rejects_image_streaming_until_the_protocol_is_supported() {
        let Err(error) = parse_request(
            ApiOperation::ImagesGeneration,
            &PreparedRequestBody::Json(Bytes::from_static(
                br#"{"model":"gpt-image-2","prompt":"test","stream":true}"#,
            )),
        ) else {
            panic!("streaming Images request should be rejected");
        };

        assert_eq!(error.code, Some("image_streaming_unsupported"));
    }

    #[test]
    fn attempted_channel_slots_spill_only_after_the_inline_retry_capacity() {
        let mut slots = AttemptedChannelSlots::new();
        for slot in 0..20 {
            slots.push(slot);
        }
        assert_eq!(slots.as_slice(), (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn extracts_session_affinity_from_ordered_header_and_json_sources() {
        let settings = affinity_settings(vec![
            SessionAffinityKeySource::RequestHeader(HeaderName::from_static("x-session-id")),
            SessionAffinityKeySource::JsonPointer(Arc::from("/id")),
        ]);
        let headers = HeaderMap::new();
        let matched = match_session_affinity(
            &settings,
            ApiFormat::OpenAiResponses,
            "gpt-5",
            &headers,
            br#"{"id":"session-body","model":"gpt-5"}"#,
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
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br"));
        headers.insert("x-request-id", HeaderValue::from_static("keep"));

        let forwarded = forward_request_headers(&headers);
        assert!(forwarded.get(CONNECTION).is_none());
        assert!(forwarded.get("x-internal-hop").is_none());
        assert!(forwarded.get("authorization").is_none());
        assert!(forwarded.get(ACCEPT_ENCODING).is_none());
        assert_eq!(forwarded.get("x-request-id").unwrap(), "keep");
    }

    #[test]
    fn removes_common_proxy_and_cdn_forwarding_metadata_from_requests() {
        let mut headers = HeaderMap::new();
        headers.insert("forwarded", HeaderValue::from_static("discard"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("discard"));
        headers.insert("cf-connecting-ip", HeaderValue::from_static("discard"));
        headers.insert(
            "x-forwarded-custom",
            HeaderValue::from_static("keep-similar-name"),
        );
        headers.insert("x-request-id", HeaderValue::from_static("keep"));

        let forwarded = forward_request_headers(&headers);

        assert!(forwarded.get("forwarded").is_none());
        assert!(forwarded.get("x-forwarded-for").is_none());
        assert!(forwarded.get("cf-connecting-ip").is_none());
        assert_eq!(
            forwarded.get("x-forwarded-custom").unwrap(),
            "keep-similar-name"
        );
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
    fn error_body_capture_excludes_binary_media_types() {
        let mut headers = HeaderMap::new();
        assert!(response_error_body_is_textual(&headers));
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/problem+json"),
        );
        assert!(response_error_body_is_textual(&headers));
        headers.insert("content-type", HeaderValue::from_static("IMAGE/PNG"));
        assert!(!response_error_body_is_textual(&headers));
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/octet-stream"),
        );
        assert!(!response_error_body_is_textual(&headers));
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
            reasoning_tokens: 1,
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
                reasoning_tokens: 1,
            }),
            2_000,
            Some(500),
        );
        assert_eq!(billing.cost_amount, Some(Decimal::new(1725, 2)));
        assert_eq!(
            billing.output_tokens_per_second,
            Some(Decimal::new(26667, 4))
        );

        let missing_ttft = request_billing(
            &snapshot,
            &CompiledAdvancedBilling::default(),
            Decimal::ONE,
            Decimal::ONE,
            Some(ResponseUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
                reasoning_tokens: 1,
            }),
            2_000,
            None,
        );
        assert_eq!(missing_ttft.output_tokens_per_second, None);
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
                reasoning_tokens: 1,
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
                output_unit_price: Some(Decimal::from(5_i64)),
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
                reasoning_tokens: 1,
            }),
            2_000,
            Some(500),
        );

        assert_eq!(billing.price.input_unit_price, Decimal::from(9_i64));
        assert_eq!(billing.price.cached_input_unit_price, Decimal::from(6_i64));
        assert_eq!(billing.price.cache_write_unit_price, Decimal::from(12_i64));
        assert_eq!(billing.price.output_unit_price, Decimal::from(15_i64));
        assert_eq!(billing.cost_amount, Some(Decimal::from(156_i64)));
    }
}
