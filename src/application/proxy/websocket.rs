//! OpenAI Responses WebSocket proxying with pinned, pooled upstream connections.

mod lifecycle;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::ws::{
        CloseFrame, Message as ClientMessage, Utf8Bytes as ClientUtf8Bytes, WebSocket, close_code,
    },
    http::{
        HeaderMap, HeaderName, HeaderValue, StatusCode, Uri,
        header::{AUTHORIZATION, CONTENT_LENGTH, HOST, PROXY_AUTHORIZATION},
    },
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::json;
use tokio::time::Sleep;
use tokio_tungstenite::tungstenite::{Message as UpstreamMessage, Utf8Bytes as UpstreamUtf8Bytes};
use zeroize::Zeroizing;

use crate::{
    admission::AdmissionError,
    application::usage::{
        PREVIOUS_RESPONSE_NOT_FOUND_CODE, SseTerminalOutcome,
        WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE,
    },
    domain::{
        ApiFormat, ApiKeyPermission, ApiOperation, CompiledApiKey, CompiledChannel,
        RequestLogSource, RequestProtocol,
    },
    request_policy::{
        RequestInterface, RequestPolicyLayer, apply_client_fast_mode_filter,
        apply_json_body_policy, filter_client_headers,
    },
    routing::{SelectionResult, SessionAffinityMatch},
    transforms::apply_websocket_event_plan,
    upstream::{
        InterruptControl, MAX_UPSTREAM_MESSAGE_BYTES, ResolvedUpstreamPolicy, UpstreamWebSocket,
        UpstreamWebSocketError, UpstreamWebSocketKey, WebSocketClientIdentity,
        connect_upstream_websocket, response_id_digest,
    },
};

use super::super::connector::PreparedUpstreamAttempt as PreparedConnectorAttempt;
use super::{
    AttemptedCandidateSlots, CompletionGuard, ProxyError, ProxyService, RequestOutcome,
    WebSocketRuntimeSnapshot,
    candidate::{self, JsonPreparationError},
    forward_request_headers, match_session_affinity, parse_bearer_token,
    request_billing_multiplier, request_log_metadata, sse_terminal_request_outcome,
};
pub(super) use lifecycle::WebSocketLifecycle;
use lifecycle::WebSocketSessionGuard;

const OPENAI_RESPONSES_FORMAT: ApiFormat = ApiFormat::OpenAiResponses;
const OPENAI_RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const DOWNSTREAM_CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const PREVIOUS_RESPONSE_NOT_FOUND_MESSAGE: &str =
    "Previous response was not found. Retrying the full request.";

/// Authenticated downstream WebSocket setup captured before HTTP upgrade.
pub(crate) struct ResponsesWebSocketSession {
    proxy: ProxyService,
    client_secret: Zeroizing<String>,
    request_headers: HeaderMap,
    request_uri: Uri,
    client_identity: WebSocketClientIdentity,
    _lifecycle_guard: WebSocketSessionGuard,
}

impl ProxyService {
    /// Authenticates a Responses WebSocket upgrade without consuming an RPM or
    /// concurrency slot. Each subsequent `response.create` is admitted and
    /// logged as an independent logical request.
    pub(crate) fn prepare_responses_websocket(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Result<ResponsesWebSocketSession, ProxyError> {
        let secret = parse_bearer_token(headers)?;
        let snapshot = self.runtime.snapshot();
        let api_key = snapshot
            .authenticate(secret)
            .ok_or_else(ProxyError::invalid_api_key)?;
        if !api_key.permits(OPENAI_RESPONSES_FORMAT, ApiKeyPermission::Proxy) {
            return Err(ProxyError::forbidden(
                "This API key cannot proxy Responses WebSocket requests.",
            ));
        }
        if !snapshot.system_settings().websocket().enabled() {
            return Err(ProxyError::websocket_unavailable());
        }
        if !api_key.websocket_enabled() {
            return Err(ProxyError::websocket_unavailable());
        }
        let lifecycle_guard = self
            .websocket_lifecycle
            .reserve()
            .ok_or_else(ProxyError::shutting_down)?;
        let mut request_headers =
            filter_client_headers(RequestInterface::ResponsesWebSocket, headers)
                .map_err(ProxyError::request_policy)?;
        // The model arrives only in response.create. Reject the upgrade early
        // only when none of this key's authorized Responses routes can use WS.
        if !self
            .routing
            .has_available_websocket_route(&snapshot, &api_key, |channel| {
                self.connectors
                    .can_attempt_responses_websocket(channel, snapshot.plugins())
            })
        {
            return Err(ProxyError::websocket_unavailable());
        }
        let identity_headers = forward_websocket_request_headers(&request_headers);
        request_headers.remove(AUTHORIZATION);
        request_headers.remove(PROXY_AUTHORIZATION);
        Ok(ResponsesWebSocketSession {
            proxy: self.clone(),
            client_secret: Zeroizing::new(secret.to_owned()),
            request_headers,
            request_uri: uri.clone(),
            client_identity: WebSocketClientIdentity::new(uri, &identity_headers),
            _lifecycle_guard: lifecycle_guard,
        })
    }

    #[must_use]
    pub(crate) fn websocket_request_limit(&self) -> usize {
        self.max_request_body_bytes
    }

    /// Stops new upgrades and asks idle Responses WebSockets to close after
    /// their current logical request.
    #[doc(hidden)]
    pub fn begin_websocket_shutdown(&self) {
        self.websocket_lifecycle.begin_draining();
        self.upstream_clients.clear_websockets();
    }

    /// Cancels any Responses WebSocket request still running after the process
    /// shutdown grace period.
    #[doc(hidden)]
    pub fn force_websocket_shutdown(&self) {
        self.websocket_lifecycle.force_close();
        self.upstream_clients.clear_websockets();
    }

    /// Number of upgrade callbacks that still own a Responses WebSocket.
    #[doc(hidden)]
    #[must_use]
    pub fn active_websocket_sessions(&self) -> usize {
        self.websocket_lifecycle.active()
    }

    #[must_use]
    pub(crate) fn websocket_runtime_snapshot(&self) -> WebSocketRuntimeSnapshot {
        let pool = self.upstream_clients.websocket_pool_snapshot();
        WebSocketRuntimeSnapshot {
            active_downstream_sessions: u64::try_from(self.websocket_lifecycle.active())
                .unwrap_or(u64::MAX),
            enabled: pool.enabled,
            idle_upstream_connections: pool.idle_connections,
            leased_upstream_connections: pool.leased_connections,
            pool_capacity: pool.capacity,
            pool_hits_total: pool.hits_total,
            pool_misses_total: pool.misses_total,
            pool_discarded_total: pool.discarded_total,
            idle_timeout_seconds: pool.idle_timeout_seconds,
            max_connection_age_seconds: pool.max_connection_age_seconds,
        }
    }

    /// Waits until all Responses WebSocket upgrade callbacks have exited.
    #[doc(hidden)]
    pub async fn wait_for_websocket_shutdown(&self) {
        self.websocket_lifecycle.wait_drained().await;
    }
}

impl ResponsesWebSocketSession {
    pub(crate) async fn run(self, mut client: WebSocket) {
        let mut next_text = None;
        let mut just_completed = None;
        loop {
            if self.proxy.websocket_lifecycle.is_draining() {
                if !self.proxy.websocket_lifecycle.is_force_closing() {
                    close_client(&mut client, close_code::AWAY, "server shutting down").await;
                }
                break;
            }
            let incoming = if let Some(text) = next_text.take() {
                Some(Ok(ClientMessage::Text(text)))
            } else {
                tokio::select! {
                    incoming = client.recv() => incoming,
                    _ = self.proxy.websocket_lifecycle.shutdown_requested() => {
                        if !self.proxy.websocket_lifecycle.is_force_closing() {
                            close_client(&mut client, close_code::AWAY, "server shutting down").await;
                        }
                        break;
                    }
                }
            };
            let Some(incoming) = incoming else {
                break;
            };
            match incoming {
                Ok(ClientMessage::Text(text)) => {
                    if self.proxy.websocket_lifecycle.is_draining() {
                        if !self.proxy.websocket_lifecycle.is_force_closing() {
                            close_client(&mut client, close_code::AWAY, "server shutting down")
                                .await;
                        }
                        break;
                    }
                    // Completion may already be on the wire when the peer interrupts its created ID.
                    if let Some(id) = matching_interrupt_id(&text, just_completed.take()) {
                        send_json(
                            &mut client,
                            json!({"type":"response.interrupt.failed", "response_id":id}),
                        )
                        .await;
                        continue;
                    }
                    let bytes: Bytes = text.into();
                    let mut pinned = None;
                    let action = tokio::select! {
                        action = self.forward_response_create(&mut client, &mut pinned, bytes) => {
                            Some(action)
                        }
                        _ = self.proxy.websocket_lifecycle.force_requested() => None,
                    };
                    self.release_pinned(pinned);
                    let Some(action) = action else {
                        break;
                    };
                    match action {
                        SessionAction::Continue => {}
                        SessionAction::JustCompleted(id) => just_completed = Some(id),
                        SessionAction::Next(text) => next_text = Some(text),
                        SessionAction::Close => break,
                    }
                }
                Ok(ClientMessage::Binary(_)) => {
                    send_error(
                        &mut client,
                        400,
                        "invalid_request_error",
                        "invalid_websocket_message",
                        "Responses WebSocket requests must be JSON text messages.",
                        None,
                    )
                    .await;
                    close_client(&mut client, close_code::UNSUPPORTED, "text frames required")
                        .await;
                    break;
                }
                Ok(ClientMessage::Ping(_) | ClientMessage::Pong(_)) => {}
                Err(error) => {
                    close_if_message_too_large(&mut client, error).await;
                    break;
                }
                Ok(ClientMessage::Close(_)) => break,
            }
        }
    }

    async fn forward_response_create(
        &self,
        client: &mut WebSocket,
        pinned: &mut Option<PinnedUpstream>,
        original_body: Bytes,
    ) -> SessionAction {
        let started_at = Instant::now();
        let started_wall_at = chrono::Utc::now();

        let snapshot = self.proxy.runtime.snapshot();
        let api_key = match snapshot.authenticate(&self.client_secret) {
            Some(api_key) => api_key,
            None => {
                send_error(
                    client,
                    401,
                    "authentication_error",
                    "invalid_api_key",
                    "Invalid or expired API key.",
                    None,
                )
                .await;
                return SessionAction::Close;
            }
        };
        if !api_key.permits(OPENAI_RESPONSES_FORMAT, ApiKeyPermission::Proxy) {
            send_error(
                client,
                403,
                "permission_error",
                "permission_denied",
                "This API key cannot proxy Responses WebSocket requests.",
                None,
            )
            .await;
            return SessionAction::Close;
        }
        if !snapshot.system_settings().websocket().enabled() {
            send_proxy_error(client, ProxyError::websocket_unavailable()).await;
            return SessionAction::Close;
        }
        if !api_key.websocket_enabled() {
            send_proxy_error(client, ProxyError::websocket_unavailable()).await;
            return SessionAction::Close;
        }
        let admission = match self.proxy.admission.admit(&api_key) {
            Ok(admission) => admission,
            Err(AdmissionError::RateLimited { retry_after }) => {
                send_error(
                    client,
                    429,
                    "rate_limit_error",
                    "rate_limit_exceeded",
                    "Request rate limit exceeded.",
                    Some(retry_after),
                )
                .await;
                return SessionAction::Close;
            }
            Err(AdmissionError::ConcurrentLimited) => {
                send_error(
                    client,
                    429,
                    "rate_limit_error",
                    "concurrent_limit_exceeded",
                    "Concurrent request limit exceeded.",
                    None,
                )
                .await;
                return SessionAction::Close;
            }
            Err(AdmissionError::InsufficientQuota) => {
                send_error(
                    client,
                    429,
                    "insufficient_quota",
                    "insufficient_quota",
                    "Quota has been exhausted.",
                    None,
                )
                .await;
                return SessionAction::Close;
            }
        };
        let applied = match apply_json_body_policy(
            RequestPolicyLayer::Client,
            RequestInterface::ResponsesWebSocket,
            original_body,
        ) {
            Ok(applied) => applied,
            Err(error) => {
                send_proxy_error(client, ProxyError::request_policy(error)).await;
                return SessionAction::Close;
            }
        };
        let filtered = match apply_client_fast_mode_filter(
            RequestInterface::ResponsesWebSocket,
            applied.body,
            api_key.filter_fast_mode(),
        ) {
            Ok(filtered) => filtered,
            Err(error) => {
                send_proxy_error(client, ProxyError::request_policy(error)).await;
                return SessionAction::Close;
            }
        };
        let original_body = filtered.body;

        let parsed = match parse_websocket_request(&original_body) {
            Ok(parsed) => parsed,
            Err(error) => {
                send_proxy_error(client, error).await;
                return SessionAction::Close;
            }
        };

        let affinity = match_session_affinity(
            snapshot.system_settings().session_affinity(),
            OPENAI_RESPONSES_FORMAT,
            &parsed.model,
            &self.request_headers,
            &original_body,
        );
        let select_route = |excluded: &[usize]| {
            self.proxy.routing.select_websocket_with_affinity_excluding(
                &snapshot,
                &api_key,
                OPENAI_RESPONSES_FORMAT,
                &parsed.model,
                affinity.clone(),
                excluded,
            )
        };
        let route = if let Some(response_id) = parsed.previous_response_id {
            *pinned = self
                .proxy
                .upstream_clients
                .acquire_websocket_continuation(api_key.id(), self.client_identity, response_id)
                .map(|(key, connection)| PinnedUpstream::new(key, connection));
            let selected = pinned.as_ref().and_then(|active| {
                self.proxy
                    .routing
                    .select_preferred_websocket_candidate(
                        &snapshot,
                        &api_key,
                        OPENAI_RESPONSES_FORMAT,
                        &parsed.model,
                        active.key.channel_id(),
                        active.key.upstream_model(),
                        affinity.clone(),
                        &[],
                    )
                    .filter(|route| {
                        route.channel.id() == active.key.channel_id()
                            && route.upstream_model == *active.key.upstream_model()
                    })
            });
            match selected {
                Some(route) => SelectionResult::Selected(route),
                None => snapshot
                    .operation_rule(ApiOperation::ResponsesWebSocket, &parsed.model)
                    .filter(|rule| api_key.permits_route(rule.route_slot()))
                    .map_or(SelectionResult::UnknownOrInaccessibleModel, |rule| {
                        SelectionResult::NoHealthyChannel { rule }
                    }),
            }
        } else {
            select_route(&[])
        };
        let crate::routing::SelectedRoute {
            rule,
            channel,
            candidate_slot,
            channel_slot: _,
            upstream_model,
            session_affinity,
            lease,
        } = match route {
            SelectionResult::Selected(route) => route,
            SelectionResult::UnknownOrInaccessibleModel => {
                if let Some(rule) = snapshot
                    .operation_rule(ApiOperation::Responses, &parsed.model)
                    .filter(|rule| api_key.permits_route(rule.route_slot()))
                {
                    let error = ProxyError::websocket_unavailable();
                    self.proxy.record_unrouted(
                        &api_key,
                        RequestLogSource::Client,
                        OPENAI_RESPONSES_FORMAT,
                        ApiOperation::ResponsesWebSocket,
                        &parsed.model,
                        &parsed.log_metadata,
                        RequestProtocol::WebSocket,
                        &rule,
                        started_wall_at,
                        started_at,
                        &error,
                    );
                    send_proxy_error(client, error).await;
                    return SessionAction::Close;
                }
                self.proxy.record_rejected(
                    &api_key,
                    RequestLogSource::Client,
                    OPENAI_RESPONSES_FORMAT,
                    ApiOperation::ResponsesWebSocket,
                    &parsed.model,
                    &parsed.log_metadata,
                    RequestProtocol::WebSocket,
                    started_wall_at,
                    started_at,
                );
                send_error(
                    client,
                    404,
                    "invalid_request_error",
                    "model_not_found",
                    "The requested model does not exist or is unavailable.",
                    None,
                )
                .await;
                return SessionAction::Close;
            }
            SelectionResult::NoHealthyChannel { rule } => {
                let error = if parsed.previous_response_id.is_some() {
                    ProxyError {
                        status: StatusCode::NOT_FOUND,
                        message: PREVIOUS_RESPONSE_NOT_FOUND_MESSAGE.to_owned(),
                        error_type: "invalid_request_error",
                        code: Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE),
                        param: None,
                        authenticate: false,
                        retry_after: None,
                    }
                } else {
                    ProxyError::websocket_unavailable()
                };
                self.proxy.record_unrouted(
                    &api_key,
                    RequestLogSource::Client,
                    OPENAI_RESPONSES_FORMAT,
                    ApiOperation::ResponsesWebSocket,
                    &parsed.model,
                    &parsed.log_metadata,
                    RequestProtocol::WebSocket,
                    &rule,
                    started_wall_at,
                    started_at,
                    &error,
                );
                if parsed.previous_response_id.is_some() {
                    send_previous_response_not_found(client).await;
                    return SessionAction::Continue;
                }
                // A 503 wrapped event is treated as terminal server overload
                // by Codex. 426 remains a transport failure, allowing its WS
                // retry budget to reach HTTP fallback without replaying here.
                send_proxy_error(client, error).await;
                return SessionAction::Close;
            }
        };

        let request_multiplier =
            request_billing_multiplier(rule.advanced_billing(), &original_body);
        let mut completion = CompletionGuard::new(
            Arc::clone(&self.proxy.request_log_sink),
            &api_key,
            RequestLogSource::Client,
            &parsed.model,
            &parsed.log_metadata,
            RequestProtocol::WebSocket,
            OPENAI_RESPONSES_FORMAT,
            ApiOperation::ResponsesWebSocket,
            &rule,
            &channel,
            &upstream_model,
            lease,
            admission,
            started_wall_at,
            started_at,
            self.proxy.automatic_disable.clone(),
            snapshot.system_settings().automatic_disable().clone(),
            session_affinity.as_ref(),
            request_multiplier,
        );
        if let Err(error) = completion.admit_request_log() {
            send_proxy_error(client, error).await;
            return SessionAction::Close;
        }
        let retry = snapshot.system_settings().request_retry();
        let max_retries = if retry.enabled() {
            retry.max_retries()
        } else {
            0
        };
        let max_attempts = max_retries.saturating_add(1);
        let mut attempt = 1_u32;
        let mut current_channel = channel;
        let mut current_candidate_slot = candidate_slot;
        let mut current_upstream_model = upstream_model;
        let mut current_session_affinity = session_affinity;
        let connector_seed = affinity
            .as_ref()
            .map(SessionAffinityMatch::session_hash)
            .unwrap_or_else(|| self.client_identity.connector_seed());
        let mut attempted_candidate_slots = AttemptedCandidateSlots::new();

        loop {
            attempted_candidate_slots.push(current_candidate_slot);
            let connector_affinity_hit = parsed.previous_response_id.is_some()
                || current_session_affinity
                    .as_ref()
                    .is_some_and(crate::routing::SessionAffinitySelection::cache_hit);
            let connector = match self.proxy.connectors.prepare(
                &current_channel,
                ApiOperation::ResponsesWebSocket,
                connector_affinity_hit,
                &self.request_headers,
                Some(connector_seed),
                snapshot.plugins(),
            ) {
                Ok(connector) => connector,
                Err(error) => {
                    if parsed.previous_response_id.is_some() {
                        return reject_previous_response_not_found(client, &mut completion).await;
                    }
                    if connector_affinity_hit {
                        completion.set_preserve_affinity_on_failure(true);
                        let error = ProxyError::sticky_connector_unavailable(error);
                        return reject_websocket_unavailable(
                            client,
                            &mut completion,
                            error.code,
                            &error.message,
                        )
                        .await;
                    }
                    let retry_route = select_route(attempted_candidate_slots.as_slice());
                    let SelectionResult::Selected(route) = retry_route else {
                        let error = ProxyError::connector_unavailable(error);
                        return reject_websocket_unavailable(
                            client,
                            &mut completion,
                            error.code,
                            &error.message,
                        )
                        .await;
                    };
                    let crate::routing::SelectedRoute {
                        rule,
                        channel,
                        candidate_slot,
                        channel_slot: _,
                        upstream_model,
                        session_affinity,
                        lease,
                    } = route;
                    let request_multiplier =
                        request_billing_multiplier(rule.advanced_billing(), &original_body);
                    completion.retry_with_route(
                        error.code(),
                        &rule,
                        &channel,
                        &upstream_model,
                        lease,
                        self.proxy.automatic_disable.clone(),
                        snapshot.system_settings().automatic_disable().clone(),
                        session_affinity.as_ref(),
                        request_multiplier,
                    );
                    current_channel = channel;
                    current_candidate_slot = candidate_slot;
                    current_upstream_model = upstream_model;
                    current_session_affinity = session_affinity;
                    continue;
                }
            };
            completion.set_preserve_affinity_on_failure(connector.preserves_affinity_on_failure());
            completion.set_usage_parser(connector.usage_parser(ApiOperation::ResponsesWebSocket));
            let prepared = match self.prepare_upstream_attempt(
                &original_body,
                &parsed,
                &api_key,
                &current_upstream_model,
                &current_channel,
                &snapshot,
                connector,
                connector_affinity_hit,
                applied.changed || filtered.changed,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    if error.status.is_server_error() {
                        return reject_websocket_unavailable(
                            client,
                            &mut completion,
                            error.code,
                            &error.message,
                        )
                        .await;
                    }
                    let outcome = if error.status.is_client_error() {
                        RequestOutcome::ClientRequestError
                    } else {
                        RequestOutcome::UpstreamUnavailable
                    };
                    completion.finish_with_proxy_error(outcome, &error);
                    send_proxy_error(client, error).await;
                    return SessionAction::Close;
                }
            };

            if !parsed.matches_prepared_continuation(&original_body, &prepared.body)
                || parsed.previous_response_id.is_some()
                    && pinned
                        .as_ref()
                        .is_none_or(|existing| existing.key != prepared.key)
            {
                return reject_previous_response_not_found(client, &mut completion).await;
            }

            completion.begin_websocket_setup();
            if pinned.is_none() {
                if let Some(connection) =
                    self.proxy.upstream_clients.acquire_websocket(&prepared.key)
                {
                    *pinned = Some(PinnedUpstream::new(prepared.key.clone(), connection));
                    completion.response_headers_received();
                } else {
                    match connect_upstream_websocket(
                        prepared.target.clone(),
                        prepared.headers.clone(),
                        current_channel.upstream_policy(),
                        prepared.policy,
                        MAX_UPSTREAM_MESSAGE_BYTES,
                    )
                    .await
                    {
                        Ok(connection) => {
                            self.proxy.upstream_clients.record_connected_websocket();
                            *pinned = Some(PinnedUpstream::new(prepared.key.clone(), connection));
                            completion.response_headers_received();
                        }
                        Err(error) => {
                            if let UpstreamWebSocketError::Http { status } = error {
                                if let Ok(status_code) = StatusCode::from_u16(status) {
                                    prepared.connector.observe_response(status_code);
                                }
                                completion.set_upstream_status(status);
                                completion.response_headers_received();
                                return reject_websocket_unavailable(
                                    client,
                                    &mut completion,
                                    Some("upstream_websocket_handshake_failed"),
                                    &format!("The upstream rejected the WebSocket handshake with HTTP {status}."),
                                )
                                .await;
                            }
                            record_connection_failure(error, &mut completion);
                            let next = (attempt < max_attempts
                                && !prepared.preserve_channel_on_failure)
                                .then(|| select_route(attempted_candidate_slots.as_slice()));
                            let Some(SelectionResult::Selected(route)) = next else {
                                return reject_websocket_unavailable(
                                    client,
                                    &mut completion,
                                    Some(connection_failure_code(error)),
                                    &format!(
                                        "The selected upstream channel could not establish a WebSocket connection: {error}"
                                    ),
                                )
                                .await;
                            };
                            let crate::routing::SelectedRoute {
                                rule,
                                channel,
                                candidate_slot,
                                channel_slot: _,
                                upstream_model,
                                session_affinity,
                                lease,
                            } = route;
                            let request_multiplier =
                                request_billing_multiplier(rule.advanced_billing(), &original_body);
                            completion.retry_with_route(
                                connection_failure_code(error),
                                &rule,
                                &channel,
                                &upstream_model,
                                lease,
                                self.proxy.automatic_disable.clone(),
                                snapshot.system_settings().automatic_disable().clone(),
                                session_affinity.as_ref(),
                                request_multiplier,
                            );
                            current_channel = channel;
                            current_candidate_slot = candidate_slot;
                            current_upstream_model = upstream_model;
                            current_session_affinity = session_affinity;
                            attempt = attempt.saturating_add(1);
                            continue;
                        }
                    }
                }
            } else {
                completion.response_headers_received();
            }

            let Some(active) = pinned.as_mut() else {
                return reject_websocket_unavailable(
                    client,
                    &mut completion,
                    Some("upstream_unavailable"),
                    "The selected upstream WebSocket connection is unavailable.",
                )
                .await;
            };
            let request = match UpstreamUtf8Bytes::try_from(prepared.body) {
                Ok(request) => request,
                Err(_) => {
                    completion.finish_with_message(
                        RequestOutcome::ClientRequestError,
                        Some("invalid_websocket_message"),
                        "Responses WebSocket requests must contain valid UTF-8 JSON.",
                    );
                    send_error(
                        client,
                        400,
                        "invalid_request_error",
                        "invalid_websocket_message",
                        "Responses WebSocket requests must contain valid UTF-8 JSON.",
                        None,
                    )
                    .await;
                    return SessionAction::Close;
                }
            };
            if parsed.previous_response_id.is_some()
                && !self
                    .proxy
                    .upstream_clients
                    .websocket_continuation_is_ready(&active.key, &mut active.connection)
            {
                return reject_previous_response_not_found(client, &mut completion).await;
            }
            if active.connection.request_too_large(request.len()) {
                return reject_upstream_size_limit(client, &mut completion).await;
            }
            completion.set_upstream_status(200);
            completion.begin_dispatch();
            match tokio::time::timeout(
                prepared.policy.timeouts().stream_idle(),
                active.connection.send(UpstreamMessage::Text(request)),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    completion.connection_failed();
                    completion.finish_with_message(
                        RequestOutcome::UpstreamUnavailable,
                        Some("upstream_unavailable"),
                        &format!(
                            "The upstream WebSocket closed before the request could be sent: {error}"
                        ),
                    );
                    send_error(
                        client,
                        502,
                        "api_error",
                        "upstream_unavailable",
                        "The upstream WebSocket closed before the request could be sent.",
                        None,
                    )
                    .await;
                    return SessionAction::Close;
                }
                Err(_) => {
                    completion.finish_with_message(
                        RequestOutcome::StreamIdleTimeout,
                        Some("stream_idle_timeout"),
                        "Sending the request to the upstream WebSocket timed out.",
                    );
                    send_error(
                        client,
                        504,
                        "api_error",
                        "stream_idle_timeout",
                        "Sending the request to the upstream WebSocket timed out.",
                        None,
                    )
                    .await;
                    return SessionAction::Close;
                }
            }

            return relay_upstream_response(
                client,
                active,
                &self.proxy.upstream_clients,
                completion,
                prepared.policy.timeouts().stream_idle(),
                current_channel
                    .upstream_policy()
                    .effective_transforms()
                    .sse_event_patches(),
                &prepared.connector,
            )
            .await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_upstream_attempt(
        &self,
        original_body: &Bytes,
        parsed: &ParsedWebSocketRequest,
        api_key: &CompiledApiKey,
        upstream_model: &str,
        channel: &CompiledChannel,
        snapshot: &crate::domain::CompiledRuntimeConfig,
        connector: PreparedConnectorAttempt,
        connector_affinity_hit: bool,
        client_body_changed: bool,
    ) -> Result<PreparedWebSocketAttempt, ProxyError> {
        let transforms = channel.upstream_policy().effective_transforms();
        let body = candidate::prepare_json(
            original_body,
            &parsed.model,
            upstream_model,
            transforms.request_json(),
            &connector,
            RequestProtocol::WebSocket,
            api_key
                .filter_fast_mode()
                .then_some(RequestInterface::ResponsesWebSocket),
        )
        .map_err(|error| match error {
            JsonPreparationError::Model(error) => error,
            JsonPreparationError::Transform => ProxyError::transform_failed(),
            JsonPreparationError::Connector(error) => ProxyError::connector_attempt(error),
        })?;
        let headers = candidate::transform_headers(
            &self.request_headers,
            client_body_changed || body != *original_body,
            transforms.request_headers(),
        )
        .map_err(|_| ProxyError::transform_failed())?;
        let mut headers = forward_websocket_request_headers(&headers);
        headers
            .entry("openai-beta")
            .or_insert(HeaderValue::from_static(OPENAI_RESPONSES_WEBSOCKET_BETA));
        connector
            .inject_headers(&mut headers, channel, RequestProtocol::WebSocket)
            .map_err(ProxyError::connector_attempt)?;
        let target = connector
            .upstream_url(channel, &self.request_uri)
            .map_err(ProxyError::connector_attempt)?;
        let target = websocket_upstream_url(target)?;
        let policy = ResolvedUpstreamPolicy::try_resolve(
            &snapshot.system_settings().upstream_timeouts(),
            channel.upstream_policy(),
        )
        .map_err(|_| ProxyError::upstream_unavailable())?;
        let key = UpstreamWebSocketKey::new(
            api_key.id(),
            self.client_identity,
            channel,
            upstream_model,
            &target,
            &headers,
            MAX_UPSTREAM_MESSAGE_BYTES,
        )
        .with_plugin_generation(connector.plugin_generation_id());
        Ok(PreparedWebSocketAttempt {
            body,
            headers,
            target,
            policy,
            key,
            preserve_channel_on_failure: connector_affinity_hit
                && connector.preserves_affinity_on_failure(),
            connector,
        })
    }

    fn release_pinned(&self, pinned: Option<PinnedUpstream>) {
        let Some(pinned) = pinned else {
            return;
        };
        if pinned.reusable && !self.proxy.websocket_lifecycle.is_draining() {
            self.proxy
                .upstream_clients
                .release_websocket(pinned.key, pinned.connection);
        } else {
            self.proxy.upstream_clients.discard_leased_websocket();
        }
    }
}

struct PreparedWebSocketAttempt {
    body: Bytes,
    headers: HeaderMap,
    target: reqwest::Url,
    policy: ResolvedUpstreamPolicy,
    key: UpstreamWebSocketKey,
    preserve_channel_on_failure: bool,
    connector: PreparedConnectorAttempt,
}

struct PinnedUpstream {
    key: UpstreamWebSocketKey,
    connection: UpstreamWebSocket,
    reusable: bool,
}

impl PinnedUpstream {
    fn new(key: UpstreamWebSocketKey, connection: UpstreamWebSocket) -> Self {
        Self {
            key,
            connection,
            reusable: false,
        }
    }
}

enum SessionAction {
    Continue,
    JustCompleted([u8; 32]),
    Next(ClientUtf8Bytes),
    Close,
}

#[derive(Deserialize)]
struct WebSocketRequestProbe<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<&'a str>,
    #[serde(borrow)]
    model: Option<&'a str>,
    #[serde(default, borrow)]
    previous_response_id: Option<std::borrow::Cow<'a, str>>,
    #[serde(default)]
    reasoning_effort: serde_json::Value,
    #[serde(default)]
    reasoning: serde_json::Value,
    #[serde(default)]
    service_tier: serde_json::Value,
}

struct ParsedWebSocketRequest {
    model: String,
    log_metadata: super::RequestLogMetadata,
    previous_response_id: Option<[u8; 32]>,
}

#[derive(Deserialize)]
struct ContinuationProbe<'a> {
    #[serde(default, borrow)]
    previous_response_id: Option<std::borrow::Cow<'a, str>>,
}

impl ParsedWebSocketRequest {
    fn matches_prepared_continuation(&self, original: &Bytes, prepared: &Bytes) -> bool {
        if std::ptr::eq(original.as_ref(), prepared.as_ref()) {
            return true;
        }
        serde_json::from_slice::<ContinuationProbe<'_>>(prepared).is_ok_and(|probe| {
            response_id_digest(probe.previous_response_id.as_deref()) == self.previous_response_id
        })
    }
}

fn parse_websocket_request(body: &[u8]) -> Result<ParsedWebSocketRequest, ProxyError> {
    let probe = serde_json::from_slice::<WebSocketRequestProbe<'_>>(body).map_err(|_| {
        ProxyError::invalid_request(
            "WebSocket message must be a response.create JSON object.",
            "body",
        )
    })?;
    if probe.kind != Some("response.create") {
        return Err(ProxyError::invalid_request(
            "WebSocket message type must be response.create.",
            "type",
        ));
    }
    let model = probe.model.map(str::trim).filter(|model| !model.is_empty());
    let Some(model) = model else {
        return Err(ProxyError::invalid_request(
            "Request body must contain a non-empty model.",
            "model",
        ));
    };
    if model.chars().count() > 300 {
        return Err(ProxyError::invalid_request(
            "Request model exceeds the supported length.",
            "model",
        ));
    }
    Ok(ParsedWebSocketRequest {
        model: model.to_owned(),
        log_metadata: request_log_metadata(
            OPENAI_RESPONSES_FORMAT,
            &probe.reasoning_effort,
            &probe.reasoning,
            &probe.service_tier,
        ),
        previous_response_id: response_id_digest(probe.previous_response_id.as_deref()),
    })
}

fn websocket_upstream_url(mut target: reqwest::Url) -> Result<reqwest::Url, ProxyError> {
    let scheme = match target.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return Err(ProxyError::upstream_unavailable()),
    };
    target
        .set_scheme(scheme)
        .map_err(|_| ProxyError::upstream_unavailable())?;
    Ok(target)
}

fn forward_websocket_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut forwarded = forward_request_headers(headers);
    forwarded.remove(HOST);
    forwarded.remove(CONTENT_LENGTH);
    forwarded.remove(AUTHORIZATION);
    forwarded.remove(PROXY_AUTHORIZATION);
    let websocket_headers = forwarded
        .keys()
        .filter(|name| name.as_str().starts_with("sec-websocket-"))
        .cloned()
        .collect::<Vec<HeaderName>>();
    for name in websocket_headers {
        forwarded.remove(name);
    }
    forwarded
}

async fn relay_upstream_response(
    client: &mut WebSocket,
    upstream: &mut PinnedUpstream,
    clients: &crate::upstream::UpstreamClientRegistry,
    mut completion: CompletionGuard,
    idle_timeout: Duration,
    response_plan: &crate::transforms::SseEventPatchPlan,
    connector: &PreparedConnectorAttempt,
) -> SessionAction {
    let idle = tokio::time::sleep(idle_timeout);
    tokio::pin!(idle);
    let mut created_id = None;
    let mut interrupt = InterruptState::None;
    let mut received_text = false;
    loop {
        tokio::select! {
            incoming = upstream.connection.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => {
                        upstream.reusable = false;
                        completion.finish_with_message(
                            RequestOutcome::UpstreamBodyError,
                            Some("upstream_websocket_closed"),
                            &format!(
                                "The upstream WebSocket failed before response.completed: {error}"
                            ),
                        );
                        send_error(
                            client,
                            502,
                            "api_error",
                            "upstream_websocket_closed",
                            "The upstream WebSocket closed before response.completed.",
                            None,
                        ).await;
                        return SessionAction::Close;
                    }
                    None => {
                        upstream.reusable = false;
                        completion.finish_with_message(
                            RequestOutcome::UpstreamBodyError,
                            Some("upstream_websocket_closed"),
                            "The upstream WebSocket closed before response.completed.",
                        );
                        send_error(
                            client,
                            502,
                            "api_error",
                            "upstream_websocket_closed",
                            "The upstream WebSocket closed before response.completed.",
                            None,
                        ).await;
                        return SessionAction::Close;
                    }
                };
                match message {
                    UpstreamMessage::Text(text) => {
                        let original: Bytes = text.into();
                        let event = inspect_event(&original);
                        if event.control.is_some_and(|control| upstream.connection.discard_detached_control(control, event.response_id)) {
                            continue;
                        }
                        if upstream.connection.has_detached_interrupt(event.response_id) {
                            completion.finish_with_message(RequestOutcome::UpstreamBodyError, Some("ambiguous_response_id"), "The upstream reused a response ID with an outstanding interrupt.");
                            close_client(client, close_code::ERROR, "ambiguous response identity").await;
                            return SessionAction::Close;
                        }
                        reset_idle(idle.as_mut(), idle_timeout);
                        received_text = true;
                        completion.record_first_byte();
                        if event.created && interrupt == InterruptState::None {
                            created_id = event.response_id;
                        }
                        let ack = event.control.filter(|control| *control != InterruptControl::Metadata);
                        if !interrupt.accepts_event(&event, created_id, matches!(connector, PreparedConnectorAttempt::Codex { .. })) {
                            completion.finish_with_message(
                                RequestOutcome::UpstreamBodyError,
                                Some("invalid_interrupt_event"),
                                "Unexpected upstream interruption event.",
                            );
                            close_client(client, close_code::ERROR, "unexpected interruption event").await;
                            return SessionAction::Close;
                        }
                        let active_interrupt = interrupt != InterruptState::None && event.response_id == created_id;
                        let terminal = completion.observe_websocket_usage(&original, active_interrupt);
                        if let Some(status) =
                            event.status.filter(|status| terminal.is_some()
                                && (100..600).contains(status) && !(200..300).contains(status))
                        {
                            completion.set_client_visible_status(status);
                            if let Ok(status_code) = StatusCode::from_u16(status) {
                                connector.observe_response(status_code);
                            }
                        }
                        if event.state_error.is_none() {
                            completion.observe_upstream_error_body(&original);
                        }
                        let transformed = match if event.state_error.is_some() || ack.is_some() {
                            Ok(original)
                        } else {
                            apply_websocket_event_plan(original, response_plan)
                        } {
                            Ok(transformed) => transformed,
                            Err(error) => {
                                upstream.reusable = false;
                                completion.finish_with_message(
                                    RequestOutcome::ResponseTransformFailed,
                                    Some("response_transform_failed"),
                                    &format!(
                                        "Upstream WebSocket event transformation failed: {error}"
                                    ),
                                );
                                send_error(
                                    client,
                                    502,
                                    "api_error",
                                    "response_transform_failed",
                                    "Upstream WebSocket event transformation failed.",
                                    None,
                                ).await;
                                return SessionAction::Close;
                            }
                        };
                        let text = match ClientUtf8Bytes::try_from(transformed) {
                            Ok(text) => text,
                            Err(_) => {
                                upstream.reusable = false;
                                completion.finish_with_message(
                                    RequestOutcome::UpstreamBodyError,
                                    Some("invalid_upstream_websocket_message"),
                                    "The upstream returned a text WebSocket message that could not be forwarded as UTF-8.",
                                );
                                return SessionAction::Close;
                            }
                        };
                        let succeeded = terminal == Some(SseTerminalOutcome::Completed)
                            && event.state_error.is_none();
                        if succeeded {
                            // Register ownership before the client can act on this terminal frame.
                            clients.observe_websocket_completion(&upstream.key, &mut upstream.connection, event.response_id);
                        }
                        if !matches!(
                            tokio::time::timeout(
                                idle_timeout,
                                client.send(ClientMessage::Text(text)),
                            )
                            .await,
                            Ok(Ok(()))
                        ) {
                            upstream.reusable = false;
                            completion.finish(RequestOutcome::Cancelled);
                            return SessionAction::Close;
                        }
                        if let Some(terminal) = terminal {
                            if succeeded && interrupt == InterruptState::Pending {
                                completion.finish(RequestOutcome::Succeeded);
                                drop(completion);
                                return relay_late_interrupt_ack(client, upstream, created_id.expect("validated interrupt response ID"), response_plan, idle_timeout).await;
                            }
                            upstream.reusable = succeeded;
                            let outcome = if event.state_error.is_some() {
                                RequestOutcome::WebSocketStateLost
                            } else {
                                sse_terminal_request_outcome(terminal, true)
                            };
                            completion.finish(outcome);
                            if succeeded && interrupt == InterruptState::None && let Some(id) = created_id && event.response_id == Some(id) {
                                return SessionAction::JustCompleted(id);
                            }
                            return SessionAction::Continue;
                        }
                        if ack.is_some() {
                            interrupt = InterruptState::Answered;
                        }
                    }
                    UpstreamMessage::Binary(_) => {
                        upstream.reusable = false;
                        completion.finish_with_message(
                            RequestOutcome::UpstreamBodyError,
                            Some("invalid_upstream_websocket_message"),
                            "The upstream returned an unsupported binary WebSocket message.",
                        );
                        send_error(
                            client,
                            502,
                            "api_error",
                            "invalid_upstream_websocket_message",
                            "The upstream returned an unsupported binary WebSocket message.",
                            None,
                        ).await;
                        return SessionAction::Close;
                    }
                    UpstreamMessage::Close(Some(frame)) if u16::from(frame.code) == close_code::SIZE && !received_text => {
                        return reject_upstream_size_limit(client, &mut completion).await;
                    }
                    UpstreamMessage::Close(_) => {
                        upstream.reusable = false;
                        completion.finish_with_message(
                            RequestOutcome::UpstreamBodyError,
                            Some("upstream_websocket_closed"),
                            "The upstream WebSocket closed before response.completed.",
                        );
                        send_error(
                            client,
                            502,
                            "api_error",
                            "upstream_websocket_closed",
                            "The upstream WebSocket closed before response.completed.",
                            None,
                        ).await;
                        return SessionAction::Close;
                    }
                    UpstreamMessage::Ping(_)
                    | UpstreamMessage::Pong(_)
                    | UpstreamMessage::Frame(_) => {}
                }
            }
            downstream = client.recv() => {
                match downstream {
                    Some(Ok(ClientMessage::Ping(_) | ClientMessage::Pong(_))) => {}
                    Some(Err(error)) => {
                        close_if_message_too_large(client, error).await;
                        completion.finish(RequestOutcome::Cancelled);
                        return SessionAction::Close;
                    }
                    Some(Ok(ClientMessage::Close(_))) | None => {
                        upstream.reusable = false;
                        completion.finish(RequestOutcome::Cancelled);
                        return SessionAction::Close;
                    }
                    Some(Ok(ClientMessage::Text(text))) if interrupt == InterruptState::None && matching_interrupt_id(&text, created_id).is_some() => {
                        if upstream.connection.request_too_large(text.len()) {
                            return reject_upstream_size_limit(client, &mut completion).await;
                        }
                        let bytes: Bytes = text.into();
                        let message = UpstreamMessage::Text(UpstreamUtf8Bytes::try_from(bytes).expect("validated UTF-8"));
                        if !matches!(tokio::time::timeout(idle_timeout, upstream.connection.send(message)).await, Ok(Ok(()))) {
                            completion.finish_with_message(
                                RequestOutcome::UpstreamBodyError,
                                Some("response_interrupt_send_failed"),
                                "Could not send the interruption to the upstream.",
                            );
                            return SessionAction::Close;
                        }
                        interrupt = InterruptState::Pending;
                    }
                    Some(Ok(ClientMessage::Text(_) | ClientMessage::Binary(_))) => {
                        upstream.reusable = false;
                        completion.finish_with_message(
                            RequestOutcome::ClientRequestError,
                            Some("websocket_request_in_progress"),
                            "Only one response.create may be in flight on a WebSocket connection.",
                        );
                        send_error(
                            client,
                            400,
                            "invalid_request_error",
                            "websocket_request_in_progress",
                            "Only one response.create may be in flight on a WebSocket connection.",
                            None,
                        ).await;
                        close_client(client, close_code::PROTOCOL, "request already in progress").await;
                        return SessionAction::Close;
                    }
                }
            }
            _ = &mut idle => {
                upstream.reusable = false;
                completion.finish_with_message(
                    RequestOutcome::StreamIdleTimeout,
                    Some("stream_idle_timeout"),
                    "The upstream WebSocket was idle for too long.",
                );
                send_error(
                    client,
                    504,
                    "api_error",
                    "stream_idle_timeout",
                    "The upstream WebSocket was idle for too long.",
                    None,
                ).await;
                return SessionAction::Close;
            }
        }
    }
}

fn reset_idle(mut idle: std::pin::Pin<&mut Sleep>, duration: Duration) {
    idle.as_mut().reset(tokio::time::Instant::now() + duration);
}

async fn send_interrupt_event(
    client: &mut WebSocket,
    bytes: Bytes,
    metadata: bool,
    response_plan: &crate::transforms::SseEventPatchPlan,
    timeout: Duration,
) -> bool {
    let bytes = if metadata {
        let Ok(bytes) = apply_websocket_event_plan(bytes, response_plan) else {
            return false;
        };
        bytes
    } else {
        bytes
    };
    let Ok(text) = ClientUtf8Bytes::try_from(bytes) else {
        return false;
    };
    matches!(
        tokio::time::timeout(timeout, client.send(ClientMessage::Text(text))).await,
        Ok(Ok(()))
    )
}

async fn relay_late_interrupt_ack(
    client: &mut WebSocket,
    upstream: &mut PinnedUpstream,
    response_id: [u8; 32],
    response_plan: &crate::transforms::SseEventPatchPlan,
    idle_timeout: Duration,
) -> SessionAction {
    let deadline = tokio::time::sleep(idle_timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            incoming = upstream.connection.next() => {
                let Some(Ok(UpstreamMessage::Text(text))) = incoming else {
                    return SessionAction::Close;
                };
                let bytes: Bytes = text.into();
                let event = inspect_event(&bytes);
                if event.control.is_some_and(|control| upstream.connection.discard_detached_control(control, event.response_id)) {
                    continue;
                }
                let Some(control) = event.control.filter(|control| control.matches(event.response_id, response_id, true)) else {
                    close_client(client, close_code::ERROR, "unexpected interruption event").await;
                    return SessionAction::Close;
                };
                if !send_interrupt_event(client, bytes, control == InterruptControl::Metadata, response_plan, idle_timeout).await {
                    return SessionAction::Close;
                }
                reset_idle(deadline.as_mut(), idle_timeout);
                if control != InterruptControl::Metadata {
                    upstream.reusable = true;
                    return SessionAction::Continue;
                }
            }
            downstream = client.recv() => match downstream {
                Some(Ok(ClientMessage::Text(text))) => {
                    // Drain and Cancel use the same interrupt. A new create after a matching
                    // terminal ends the caller's wait, but its old ACK can still arrive late.
                    upstream.reusable = upstream.connection.detach_interrupt(response_id);
                    return SessionAction::Next(text);
                }
                Some(Ok(ClientMessage::Ping(_) | ClientMessage::Pong(_))) => {},
                Some(Err(error)) => { close_if_message_too_large(client, error).await; return SessionAction::Close; }
                _ => return SessionAction::Close,
            },
            _ = &mut deadline => {
                close_client(client, close_code::ERROR, "interruption acknowledgement timed out").await;
                return SessionAction::Close;
            }
        }
    }
}

async fn close_if_message_too_large(client: &mut WebSocket, error: axum::Error) {
    if matches!(
        error.into_inner().downcast_ref::<tungstenite_axum::Error>(),
        Some(tungstenite_axum::Error::Capacity(
            tungstenite_axum::error::CapacityError::MessageTooLong { .. }
        ))
    ) {
        close_client(client, close_code::SIZE, "request too large").await;
    }
}

async fn reject_upstream_size_limit(
    client: &mut WebSocket,
    completion: &mut CompletionGuard,
) -> SessionAction {
    completion.set_client_visible_status(413);
    completion.finish_with_message(
        RequestOutcome::ClientRequestError,
        Some("websocket_message_too_large"),
        "The request exceeds the upstream WebSocket message limit.",
    );
    close_client(client, close_code::SIZE, "request too large").await;
    SessionAction::Close
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InterruptState {
    None,
    Pending,
    Answered,
}

impl InterruptState {
    fn accepts_event(
        self,
        event: &EventInspection,
        created_id: Option<[u8; 32]>,
        codex: bool,
    ) -> bool {
        if self != Self::None && !event.parsed {
            false
        } else if matches!(
            event.control,
            Some(InterruptControl::Accepted | InterruptControl::Rejected)
        ) {
            self == Self::Pending && event.response_id == created_id
        } else if self != Self::None
            && (event.completion || (!codex && event.cancelled) || event.response_id.is_some())
        {
            event.response_id == created_id
        } else {
            true
        }
    }
}

fn matching_interrupt_id(
    text: &str,
    created_id: Option<[u8; 32]>,
) -> Option<std::borrow::Cow<'_, str>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Interrupt<'a> {
        #[serde(rename = "type")]
        kind: &'a str,
        mode: &'a str,
        #[serde(borrow)]
        response_id: std::borrow::Cow<'a, str>,
    }
    let id = created_id?;
    let frame = serde_json::from_str::<Interrupt<'_>>(text).ok()?;
    (frame.kind == "response.interrupt"
        && frame.mode == "discard_partial_items"
        && response_id_digest(Some(&frame.response_id)) == Some(id))
    .then_some(frame.response_id)
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum WebSocketStateError {
    #[default]
    None,
    ConnectionLimitReached,
    PreviousResponseNotFound,
}

impl WebSocketStateError {
    const fn is_none(self) -> bool {
        matches!(self, Self::None)
    }

    const fn is_some(self) -> bool {
        !self.is_none()
    }
}

#[derive(Default)]
struct EventInspection {
    parsed: bool,
    created: bool,
    completion: bool,
    cancelled: bool,
    control: Option<InterruptControl>,
    status: Option<u16>,
    state_error: WebSocketStateError,
    response_id: Option<[u8; 32]>,
}

#[derive(Deserialize)]
struct EventInspectionProbe<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<&'a str>,
    #[serde(default, borrow)]
    response_id: Option<std::borrow::Cow<'a, str>>,
    #[serde(default, alias = "status_code")]
    status: Option<u16>,
    #[serde(default, borrow)]
    error: Option<EventErrorProbe<'a>>,
    #[serde(default, borrow)]
    response: Option<&'a serde_json::value::RawValue>,
}

#[derive(Deserialize)]
struct EventResponseProbe<'a> {
    #[serde(default, borrow)]
    id: Option<std::borrow::Cow<'a, str>>,
}

#[derive(Deserialize)]
struct EventErrorProbe<'a> {
    #[serde(borrow)]
    code: Option<&'a str>,
}

fn inspect_event(bytes: &[u8]) -> EventInspection {
    let Ok(probe) = serde_json::from_slice::<EventInspectionProbe<'_>>(bytes) else {
        return EventInspection::default();
    };
    let state_error = if probe.kind == Some("error") {
        match probe.error.and_then(|error| error.code) {
            Some(WEBSOCKET_CONNECTION_LIMIT_REACHED_CODE) => {
                WebSocketStateError::ConnectionLimitReached
            }
            Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE) => WebSocketStateError::PreviousResponseNotFound,
            _ => WebSocketStateError::None,
        }
    } else {
        WebSocketStateError::None
    };
    let control = InterruptControl::from_kind(probe.kind);
    let completion = matches!(
        probe.kind,
        Some("response.completed" | "response.incomplete" | "response.failed")
    );
    EventInspection {
        parsed: true,
        created: probe.kind == Some("response.created"),
        completion,
        cancelled: probe.kind == Some("response.cancelled"),
        control,
        status: probe.status,
        state_error,
        response_id: if control.is_some() {
            response_id_digest(probe.response_id.as_deref())
        } else if completion || probe.response.is_some() {
            probe
                .response
                .and_then(|response| {
                    serde_json::from_str::<EventResponseProbe<'_>>(response.get()).ok()
                })
                .and_then(|response| response_id_digest(response.id.as_deref()))
        } else {
            response_id_digest(probe.response_id.as_deref())
        },
    }
}

fn record_connection_failure(error: UpstreamWebSocketError, completion: &mut CompletionGuard) {
    match error {
        UpstreamWebSocketError::ConnectTimeout | UpstreamWebSocketError::Network => {
            completion.connection_failed();
        }
        UpstreamWebSocketError::HandshakeTimeout => completion.probe_failed(),
        UpstreamWebSocketError::InvalidConfiguration
        | UpstreamWebSocketError::Http { .. }
        | UpstreamWebSocketError::Closed => {}
    }
}

fn connection_failure_code(error: UpstreamWebSocketError) -> &'static str {
    match error {
        UpstreamWebSocketError::ConnectTimeout => "connect_timeout",
        UpstreamWebSocketError::HandshakeTimeout => "response_header_timeout",
        UpstreamWebSocketError::InvalidConfiguration
        | UpstreamWebSocketError::Network
        | UpstreamWebSocketError::Closed
        | UpstreamWebSocketError::Http { .. } => "upstream_unavailable",
    }
}

async fn reject_websocket_unavailable(
    client: &mut WebSocket,
    completion: &mut CompletionGuard,
    diagnostic_code: Option<&str>,
    diagnostic_message: &str,
) -> SessionAction {
    completion.set_client_visible_status(StatusCode::UPGRADE_REQUIRED.as_u16());
    completion.finish_with_message(
        RequestOutcome::WebSocketUnavailable,
        diagnostic_code,
        diagnostic_message,
    );
    send_proxy_error(client, ProxyError::websocket_unavailable()).await;
    SessionAction::Close
}

async fn reject_previous_response_not_found(
    client: &mut WebSocket,
    completion: &mut CompletionGuard,
) -> SessionAction {
    completion.set_client_visible_status(StatusCode::NOT_FOUND.as_u16());
    completion.finish_with_message(
        RequestOutcome::WebSocketStateLost,
        Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE),
        PREVIOUS_RESPONSE_NOT_FOUND_MESSAGE,
    );
    send_previous_response_not_found(client).await;
    SessionAction::Continue
}

async fn send_previous_response_not_found(client: &mut WebSocket) {
    send_json(
        client,
        json!({
            "type": "error",
            "status": StatusCode::NOT_FOUND.as_u16(),
            "error": {
                "type": "invalid_request_error",
                "code": PREVIOUS_RESPONSE_NOT_FOUND_CODE,
                "message": PREVIOUS_RESPONSE_NOT_FOUND_MESSAGE,
            }
        }),
    )
    .await;
}

async fn send_proxy_error(client: &mut WebSocket, error: ProxyError) {
    send_error(
        client,
        error.status.as_u16(),
        error.error_type,
        error.code.unwrap_or("invalid_request"),
        &error.message,
        error.retry_after,
    )
    .await;
}

async fn send_error(
    client: &mut WebSocket,
    status: u16,
    error_type: &'static str,
    code: &'static str,
    message: &str,
    retry_after: Option<u64>,
) {
    let mut headers = serde_json::Map::new();
    if let Some(retry_after) = retry_after {
        headers.insert("retry-after".into(), json!(retry_after));
    }
    let payload = json!({
        "type": "error",
        "status": status,
        "error": {
            "type": error_type,
            "code": code,
            "message": message,
        },
        "headers": headers,
    });
    send_json(client, payload).await;
}

async fn send_json(client: &mut WebSocket, payload: serde_json::Value) {
    if let Ok(payload) = serde_json::to_string(&payload) {
        let _ = tokio::time::timeout(
            DOWNSTREAM_CONTROL_WRITE_TIMEOUT,
            client.send(ClientMessage::Text(payload.into())),
        )
        .await;
    }
}

async fn close_client(client: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = tokio::time::timeout(
        DOWNSTREAM_CONTROL_WRITE_TIMEOUT,
        client.send(ClientMessage::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await;
}
