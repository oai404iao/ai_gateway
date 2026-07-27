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
        HeaderMap, HeaderName, HeaderValue, Uri,
        header::{AUTHORIZATION, CONTENT_LENGTH, HOST, PROXY_AUTHORIZATION},
    },
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::json;
use tokio::time::Sleep;
use tokio_tungstenite::tungstenite::{Message as UpstreamMessage, Utf8Bytes as UpstreamUtf8Bytes};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    admission::AdmissionError,
    domain::{
        ApiFormat, ApiKeyPermission, CompiledApiKey, CompiledChannel, CompiledModelRule,
        MAX_REQUEST_RETRIES,
    },
    routing::{SelectionResult, SessionAffinityMatch},
    transforms::{apply_header_plan, apply_json_patch_plan, apply_websocket_event_plan},
    upstream::{
        MAX_UPSTREAM_MESSAGE_BYTES, ResolvedUpstreamPolicy, UpstreamWebSocket,
        UpstreamWebSocketError, UpstreamWebSocketKey, WebSocketClientIdentity,
        connect_upstream_websocket,
    },
};

use super::{
    CompletionGuard, ProxyError, ProxyService, RequestOutcome, forward_request_headers,
    inject_upstream_auth, match_session_affinity, parse_bearer_token, request_billing_multiplier,
    rewrite_model_alias, sse_terminal_request_outcome, upstream_url,
};
pub(super) use lifecycle::WebSocketLifecycle;
use lifecycle::WebSocketSessionGuard;

const OPENAI_RESPONSES_FORMAT: ApiFormat = ApiFormat::OpenAiResponses;
const OPENAI_RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const DOWNSTREAM_CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

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
        let lifecycle_guard = self
            .websocket_lifecycle
            .reserve()
            .ok_or_else(ProxyError::shutting_down)?;
        let identity_headers = forward_websocket_request_headers(headers);
        let mut request_headers = headers.clone();
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

    /// Waits until all Responses WebSocket upgrade callbacks have exited.
    #[doc(hidden)]
    pub async fn wait_for_websocket_shutdown(&self) {
        self.websocket_lifecycle.wait_drained().await;
    }
}

impl ResponsesWebSocketSession {
    pub(crate) async fn run(self, mut client: WebSocket) {
        let mut pinned = None;
        loop {
            if self.proxy.websocket_lifecycle.is_draining() {
                if !self.proxy.websocket_lifecycle.is_force_closing() {
                    close_client(&mut client, close_code::AWAY, "server shutting down").await;
                }
                break;
            }
            let incoming = tokio::select! {
                incoming = client.recv() => incoming,
                _ = self.proxy.websocket_lifecycle.shutdown_requested() => {
                    if !self.proxy.websocket_lifecycle.is_force_closing() {
                        close_client(&mut client, close_code::AWAY, "server shutting down").await;
                    }
                    break;
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
                    let bytes: Bytes = text.into();
                    let action = tokio::select! {
                        action = self.forward_response_create(&mut client, &mut pinned, bytes) => {
                            Some(action)
                        }
                        _ = self.proxy.websocket_lifecycle.force_requested() => None,
                    };
                    let Some(action) = action else {
                        break;
                    };
                    match action {
                        SessionAction::Continue => {}
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
                Ok(ClientMessage::Close(_)) | Err(_) => break,
            }
        }
        self.release_pinned(pinned);
    }

    async fn forward_response_create(
        &self,
        client: &mut WebSocket,
        pinned: &mut Option<PinnedUpstream>,
        original_body: Bytes,
    ) -> SessionAction {
        let started_at = Instant::now();
        let started_wall_at = chrono::Utc::now();

        if pinned
            .as_ref()
            .is_some_and(|pinned| !pinned.reusable || pinned.connection.is_closed())
        {
            pinned.take();
        }

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
        let preferred_channel = pinned
            .as_ref()
            .map(|pinned| pinned.key.channel_id())
            .or_else(|| {
                self.proxy
                    .upstream_clients
                    .preferred_websocket_channel(api_key.id(), self.client_identity)
            });
        let route = select_websocket_route(
            &self.proxy,
            &snapshot,
            &api_key,
            &parsed.model,
            affinity.clone(),
            preferred_channel,
            &[],
        );
        let crate::routing::SelectedRoute {
            rule,
            channel,
            channel_slot,
            session_affinity,
            lease,
        } = match route {
            SelectionResult::Selected(route) => route,
            SelectionResult::UnknownOrInaccessibleModel => {
                self.proxy.record_rejected(
                    &api_key,
                    OPENAI_RESPONSES_FORMAT,
                    &parsed.model,
                    true,
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
                self.proxy.record_no_healthy_channel(
                    &api_key,
                    OPENAI_RESPONSES_FORMAT,
                    &parsed.model,
                    true,
                    &rule,
                    started_wall_at,
                    started_at,
                );
                send_error(
                    client,
                    503,
                    "api_error",
                    "no_healthy_channel",
                    "No healthy upstream channel is currently available for this model.",
                    None,
                )
                .await;
                return SessionAction::Close;
            }
        };

        let request_multiplier =
            request_billing_multiplier(rule.advanced_billing(), &original_body);
        let mut completion = CompletionGuard::new(
            Arc::clone(&self.proxy.request_log_sink),
            &api_key,
            &parsed.model,
            true,
            OPENAI_RESPONSES_FORMAT,
            &rule,
            &channel,
            lease,
            admission,
            started_wall_at,
            started_at,
            self.proxy.automatic_disable.clone(),
            snapshot.system_settings().automatic_disable().clone(),
            session_affinity.as_ref(),
            request_multiplier,
        );
        let retry = snapshot.system_settings().request_retry();
        let max_retries = if retry.enabled() {
            retry.max_retries()
        } else {
            0
        };
        let max_attempts = max_retries.saturating_add(1);
        let mut attempted_channel_slots = [usize::MAX; MAX_REQUEST_RETRIES as usize + 1];
        let mut attempted_channel_count = 0_usize;
        let mut attempt = 1_u32;
        let mut current_rule = rule;
        let mut current_channel = channel;
        let mut current_channel_slot = channel_slot;

        loop {
            attempted_channel_slots[attempted_channel_count] = current_channel_slot;
            attempted_channel_count += 1;
            let prepared = match self.prepare_upstream_attempt(
                &original_body,
                &parsed,
                &api_key,
                &current_rule,
                &current_channel,
                &snapshot,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    completion.finish(RequestOutcome::ClientRequestError);
                    send_proxy_error(client, error).await;
                    return SessionAction::Close;
                }
            };

            if let Some(existing) = pinned.as_ref()
                && existing.key != prepared.key
                && parsed.previous_response_id
            {
                completion.finish(RequestOutcome::ClientRequestError);
                send_error(
                    client,
                    400,
                    "invalid_request_error",
                    "previous_response_not_found",
                    "Previous response state is unavailable on the selected upstream connection.",
                    None,
                )
                .await;
                return SessionAction::Close;
            }
            if pinned
                .as_ref()
                .is_some_and(|existing| existing.key != prepared.key)
            {
                self.release_pinned(pinned.take());
            }

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
                            *pinned = Some(PinnedUpstream::new(prepared.key.clone(), connection));
                            completion.response_headers_received();
                        }
                        Err(error) => {
                            if let UpstreamWebSocketError::Http { status } = error {
                                completion.set_upstream_status(status);
                                completion.response_headers_received();
                                completion.finish(RequestOutcome::UpstreamHttpError);
                                send_error(
                                    client,
                                    status,
                                    "api_error",
                                    "upstream_websocket_handshake_failed",
                                    "The upstream rejected the WebSocket handshake.",
                                    None,
                                )
                                .await;
                                return SessionAction::Close;
                            }
                            record_connection_failure(error, &mut completion);
                            let next = (attempt < max_attempts).then(|| {
                                select_websocket_route(
                                    &self.proxy,
                                    &snapshot,
                                    &api_key,
                                    &parsed.model,
                                    affinity.clone(),
                                    None,
                                    &attempted_channel_slots[..attempted_channel_count],
                                )
                            });
                            let Some(SelectionResult::Selected(route)) = next else {
                                let (outcome, status, code) = connection_failure_response(error);
                                completion.finish(outcome);
                                send_error(
                                    client,
                                    status,
                                    "api_error",
                                    code,
                                    "The selected upstream channel could not establish a WebSocket connection.",
                                    None,
                                )
                                .await;
                                return SessionAction::Close;
                            };
                            let crate::routing::SelectedRoute {
                                rule,
                                channel,
                                channel_slot,
                                session_affinity,
                                lease,
                            } = route;
                            let request_multiplier =
                                request_billing_multiplier(rule.advanced_billing(), &original_body);
                            completion.retry_with_route(
                                &rule,
                                &channel,
                                lease,
                                self.proxy.automatic_disable.clone(),
                                snapshot.system_settings().automatic_disable().clone(),
                                session_affinity.as_ref(),
                                request_multiplier,
                            );
                            tracing::warn!(
                                event = "proxy_websocket_retry",
                                api_key_id = %api_key.id(),
                                client_model = %parsed.model,
                                failed_channel_id = %current_channel.id(),
                                next_channel_id = %channel.id(),
                                attempt = attempt.saturating_add(1),
                                max_retries,
                                "retrying Responses WebSocket setup on another channel"
                            );
                            current_rule = rule;
                            current_channel = channel;
                            current_channel_slot = channel_slot;
                            attempt = attempt.saturating_add(1);
                            continue;
                        }
                    }
                }
            } else {
                completion.response_headers_received();
            }

            let Some(active) = pinned.as_mut() else {
                completion.finish(RequestOutcome::UpstreamUnavailable);
                return SessionAction::Close;
            };
            active.reusable = false;
            completion.set_upstream_status(200);
            let request = match UpstreamUtf8Bytes::try_from(prepared.body) {
                Ok(request) => request,
                Err(_) => {
                    completion.finish(RequestOutcome::ClientRequestError);
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
            match tokio::time::timeout(
                prepared.policy.timeouts().stream_idle(),
                active.connection.send(UpstreamMessage::Text(request)),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    completion.connection_failed();
                    completion.finish(RequestOutcome::UpstreamUnavailable);
                    pinned.take();
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
                    completion.finish(RequestOutcome::StreamIdleTimeout);
                    pinned.take();
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

            let action = relay_upstream_response(
                client,
                active,
                completion,
                prepared.policy.timeouts().stream_idle(),
                current_channel
                    .upstream_policy()
                    .effective_transforms()
                    .sse_event_patches(),
            )
            .await;
            if active.reusable {
                self.release_pinned(pinned.take());
            } else {
                pinned.take();
            }
            return action;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_upstream_attempt(
        &self,
        original_body: &Bytes,
        parsed: &ParsedWebSocketRequest,
        api_key: &CompiledApiKey,
        rule: &CompiledModelRule,
        channel: &CompiledChannel,
        snapshot: &crate::domain::CompiledRuntimeConfig,
    ) -> Result<PreparedUpstreamAttempt, ProxyError> {
        let transforms = channel.upstream_policy().effective_transforms();
        let body = rewrite_model_alias(original_body.clone(), &parsed.model, rule)?;
        let body = apply_json_patch_plan(body, transforms.request_json())
            .map_err(|_| ProxyError::transform_failed())?;
        let mut headers = self.request_headers.clone();
        apply_header_plan(&mut headers, transforms.request_headers())
            .map_err(|_| ProxyError::transform_failed())?;
        let mut headers = forward_websocket_request_headers(&headers);
        headers
            .entry("openai-beta")
            .or_insert(HeaderValue::from_static(OPENAI_RESPONSES_WEBSOCKET_BETA));
        inject_upstream_auth(&mut headers, channel)?;
        let target = websocket_upstream_url(channel, &self.request_uri)?;
        let policy = ResolvedUpstreamPolicy::try_resolve(
            &snapshot.system_settings().upstream_timeouts(),
            channel.upstream_policy(),
        )
        .map_err(|_| ProxyError::upstream_unavailable())?;
        let key = UpstreamWebSocketKey::new(
            api_key.id(),
            self.client_identity,
            channel,
            &target,
            &headers,
            MAX_UPSTREAM_MESSAGE_BYTES,
        );
        Ok(PreparedUpstreamAttempt {
            body,
            headers,
            target,
            policy,
            key,
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
        }
    }
}

struct PreparedUpstreamAttempt {
    body: Bytes,
    headers: HeaderMap,
    target: reqwest::Url,
    policy: ResolvedUpstreamPolicy,
    key: UpstreamWebSocketKey,
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
    Close,
}

#[derive(Deserialize)]
struct WebSocketRequestProbe<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<&'a str>,
    #[serde(borrow)]
    model: Option<&'a str>,
    #[serde(default)]
    previous_response_id: Option<&'a str>,
}

struct ParsedWebSocketRequest {
    model: String,
    previous_response_id: bool,
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
        previous_response_id: probe
            .previous_response_id
            .is_some_and(|value| !value.is_empty()),
    })
}

fn select_websocket_route(
    proxy: &ProxyService,
    snapshot: &crate::domain::CompiledRuntimeConfig,
    api_key: &CompiledApiKey,
    model: &str,
    affinity: Option<SessionAffinityMatch>,
    preferred_channel: Option<Uuid>,
    excluded_channel_slots: &[usize],
) -> SelectionResult {
    if let Some(preferred_channel) = preferred_channel
        && let Some(selected) = proxy.routing.select_preferred_channel(
            snapshot,
            api_key,
            OPENAI_RESPONSES_FORMAT,
            model,
            preferred_channel,
            affinity.clone(),
            excluded_channel_slots,
        )
    {
        return SelectionResult::Selected(selected);
    }
    proxy.routing.select_with_affinity_excluding(
        snapshot,
        api_key,
        OPENAI_RESPONSES_FORMAT,
        model,
        affinity,
        excluded_channel_slots,
    )
}

fn websocket_upstream_url(
    channel: &CompiledChannel,
    uri: &Uri,
) -> Result<reqwest::Url, ProxyError> {
    let mut target = upstream_url(channel, uri)?;
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
    mut completion: CompletionGuard,
    idle_timeout: Duration,
    response_plan: &crate::transforms::SseEventPatchPlan,
) -> SessionAction {
    let idle = tokio::time::sleep(idle_timeout);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            incoming = upstream.connection.next() => {
                reset_idle(idle.as_mut(), idle_timeout);
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(_)) | None => {
                        upstream.reusable = false;
                        completion.finish(RequestOutcome::UpstreamBodyError);
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
                        completion.record_first_byte();
                        let terminal = completion.observe_websocket_usage(&original);
                        let event = terminal
                            .map(|_| inspect_event(&original))
                            .unwrap_or_default();
                        if let Some(status) =
                            event.status.filter(|status| {
                                (100..600).contains(status) && !(200..300).contains(status)
                            })
                        {
                            completion.set_client_visible_status(status);
                        }
                        if !event.connection_limit_reached {
                            completion.observe_upstream_error_body(&original);
                        }
                        let transformed = match apply_websocket_event_plan(
                            original,
                            response_plan,
                        ) {
                            Ok(transformed) => transformed,
                            Err(_) => {
                                upstream.reusable = false;
                                completion.finish(RequestOutcome::ResponseTransformFailed);
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
                                completion.finish(RequestOutcome::UpstreamBodyError);
                                return SessionAction::Close;
                            }
                        };
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
                            let succeeded = matches!(
                                terminal,
                                crate::application::usage::SseTerminalOutcome::Completed
                            );
                            upstream.reusable = succeeded && !event.connection_limit_reached;
                            completion.finish(sse_terminal_request_outcome(terminal, true));
                            return SessionAction::Continue;
                        }
                    }
                    UpstreamMessage::Binary(_) => {
                        upstream.reusable = false;
                        completion.finish(RequestOutcome::UpstreamBodyError);
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
                    UpstreamMessage::Close(_) => {
                        upstream.reusable = false;
                        completion.finish(RequestOutcome::UpstreamBodyError);
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
                    Some(Ok(ClientMessage::Close(_))) | Some(Err(_)) | None => {
                        upstream.reusable = false;
                        completion.finish(RequestOutcome::Cancelled);
                        return SessionAction::Close;
                    }
                    Some(Ok(ClientMessage::Text(_) | ClientMessage::Binary(_))) => {
                        upstream.reusable = false;
                        completion.finish(RequestOutcome::ClientRequestError);
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
                completion.finish(RequestOutcome::StreamIdleTimeout);
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

#[derive(Default)]
struct EventInspection {
    status: Option<u16>,
    connection_limit_reached: bool,
}

#[derive(Deserialize)]
struct EventInspectionProbe<'a> {
    #[serde(default, alias = "status_code")]
    status: Option<u16>,
    #[serde(default, borrow)]
    error: Option<EventErrorProbe<'a>>,
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
    let connection_limit_reached =
        probe.error.and_then(|error| error.code) == Some("websocket_connection_limit_reached");
    EventInspection {
        status: probe.status,
        connection_limit_reached,
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

fn connection_failure_response(
    error: UpstreamWebSocketError,
) -> (RequestOutcome, u16, &'static str) {
    match error {
        UpstreamWebSocketError::ConnectTimeout => {
            (RequestOutcome::ConnectTimeout, 504, "connect_timeout")
        }
        UpstreamWebSocketError::HandshakeTimeout => (
            RequestOutcome::ResponseHeaderTimeout,
            504,
            "response_header_timeout",
        ),
        UpstreamWebSocketError::InvalidConfiguration
        | UpstreamWebSocketError::Network
        | UpstreamWebSocketError::Closed
        | UpstreamWebSocketError::Http { .. } => (
            RequestOutcome::UpstreamUnavailable,
            502,
            "upstream_unavailable",
        ),
    }
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
