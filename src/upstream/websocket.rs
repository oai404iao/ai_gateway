//! Bounded, connection-local Responses WebSocket reuse and message pumping.

mod dialer;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, Uri};
use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{
    Message, extensions::compression::deflate::DeflateConfig, protocol::WebSocketConfig,
};
use uuid::Uuid;

use crate::domain::{
    CompiledChannel, CompiledChannelUpstreamPolicy, CompiledRuntimeConfig, ConnectorKind,
    OutboundNetworkPolicyFingerprint, ResponsesWebSocketSettings,
};

use super::ResolvedUpstreamPolicy;

const COMMAND_CAPACITY: usize = 8;
const MESSAGE_CAPACITY: usize = 16;
const REAPER_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const MAX_UPSTREAM_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const WEBSOCKET_MAX_MESSAGE_HEADER: &str = "x-codex-websocket-max-message-bytes";

/// Opaque identity for the downstream handshake headers and request URI.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct WebSocketClientIdentity([u8; 32]);

impl WebSocketClientIdentity {
    #[must_use]
    pub(crate) fn new(uri: &Uri, headers: &HeaderMap) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"ai-gateway/websocket-client-identity/v1\0");
        hash_value(&mut hasher, uri.path().as_bytes());
        hash_value(&mut hasher, uri.query().unwrap_or_default().as_bytes());
        hash_headers(&mut hasher, headers);
        Self(hasher.finalize().into())
    }

    #[must_use]
    pub(crate) const fn connector_seed(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for WebSocketClientIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebSocketClientIdentity(REDACTED)")
    }
}

/// Credential-free key for one reusable upstream Responses WebSocket.
#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct UpstreamWebSocketKey {
    api_key_id: Uuid,
    client_identity: WebSocketClientIdentity,
    channel_id: Uuid,
    connector_kind: ConnectorKind,
    plugin_generation: Option<Arc<str>>,
    upstream_model: Arc<str>,
    connectivity_fingerprint: Arc<str>,
    outbound_network_policy_fingerprint: OutboundNetworkPolicyFingerprint,
    target: Arc<str>,
    header_fingerprint: [u8; 32],
    max_message_bytes: usize,
}

impl UpstreamWebSocketKey {
    #[must_use]
    pub(crate) fn new(
        api_key_id: Uuid,
        client_identity: WebSocketClientIdentity,
        channel: &CompiledChannel,
        upstream_model: &str,
        target: &Url,
        headers: &HeaderMap,
        max_message_bytes: usize,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"ai-gateway/upstream-websocket-headers/v1\0");
        hash_headers(&mut hasher, headers);
        Self {
            api_key_id,
            client_identity,
            channel_id: channel.id(),
            connector_kind: channel.connector_kind(),
            plugin_generation: None,
            upstream_model: Arc::from(upstream_model),
            connectivity_fingerprint: Arc::clone(channel.connectivity_fingerprint()),
            outbound_network_policy_fingerprint: channel
                .upstream_policy()
                .outbound_network_policy_fingerprint(),
            target: Arc::from(target.as_str()),
            header_fingerprint: hasher.finalize().into(),
            max_message_bytes,
        }
    }

    #[must_use]
    pub(crate) fn with_plugin_generation(mut self, generation: Option<&str>) -> Self {
        self.plugin_generation = generation.map(Arc::from);
        self
    }

    #[must_use]
    pub(crate) fn channel_id(&self) -> Uuid {
        self.channel_id
    }

    #[must_use]
    pub(crate) fn upstream_model(&self) -> &Arc<str> {
        &self.upstream_model
    }
}

impl fmt::Debug for UpstreamWebSocketKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamWebSocketKey")
            .field("api_key_id", &self.api_key_id)
            .field("client_identity", &self.client_identity)
            .field("channel_id", &self.channel_id)
            .field("connector_kind", &self.connector_kind)
            .field("target", &"<redacted>")
            .field("headers", &"<redacted>")
            .field("max_message_bytes", &self.max_message_bytes)
            .finish()
    }
}

fn hash_headers(hasher: &mut Sha256, headers: &HeaderMap) {
    let mut values = headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes(), value.as_bytes()))
        .collect::<Vec<_>>();
    values.sort_unstable();
    hasher.update((values.len() as u64).to_be_bytes());
    for (name, value) in values {
        hash_value(hasher, name);
        hash_value(hasher, value);
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    use crate::domain::{
        ApiFormat, ApiOperation, ChannelIdentity, RequestCompression, UpstreamAuth,
    };

    fn key(credential: Uuid, revision: Uuid) -> UpstreamWebSocketKey {
        key_with_connector(ConnectorKind::OpenAiCompatible, Some(credential), revision)
    }

    fn key_with_connector(
        connector: ConnectorKind,
        credential: Option<Uuid>,
        revision: Uuid,
    ) -> UpstreamWebSocketKey {
        let channel = CompiledChannel::new_with_connector_policy_automation_and_billing(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            ApiFormat::OpenAiResponses,
            connector,
            RequestCompression::Default,
            Url::parse("https://upstream.test").unwrap(),
            rust_decimal::Decimal::ONE,
            UpstreamAuth::Bearer(Arc::from("same-test-secret")),
            HashSet::new(),
            true,
            false,
            false,
            false,
            None,
            CompiledChannelUpstreamPolicy::transparent(ApiFormat::OpenAiResponses),
        )
        .with_channel_identity(ChannelIdentity {
            logical_channel_id: Uuid::from_u128(1),
            access_id: Uuid::from_u128(2),
            api_operation: ApiOperation::ResponsesWebSocket,
            credential_id: credential,
            credential_revision: credential.map(|_| revision),
            binding_revision: Uuid::nil(),
            access_revision: Uuid::nil(),
            capability_revision: Uuid::nil(),
        });
        UpstreamWebSocketKey::new(
            Uuid::from_u128(3),
            WebSocketClientIdentity::new(&"/v1/responses".parse().unwrap(), &HeaderMap::new()),
            &channel,
            "test-model",
            &Url::parse("wss://upstream.test/v1/responses").unwrap(),
            &HeaderMap::new(),
            1024,
        )
    }

    #[test]
    fn identical_secrets_do_not_share_identity_or_revision() {
        let original = key(Uuid::from_u128(4), Uuid::from_u128(5));
        assert_ne!(original, key(Uuid::from_u128(6), Uuid::from_u128(5)));
        assert_ne!(original, key(Uuid::from_u128(4), Uuid::from_u128(7)));
    }

    #[test]
    fn connector_identity_isolates_even_legacy_channels_with_identical_auth_and_target() {
        let first = key_with_connector(
            ConnectorKind::parse("plugin-first").unwrap(),
            None,
            Uuid::nil(),
        );
        let second = key_with_connector(
            ConnectorKind::parse("plugin-second").unwrap(),
            None,
            Uuid::nil(),
        );
        assert_eq!(
            first.connectivity_fingerprint,
            second.connectivity_fingerprint
        );
        assert_eq!(first.header_fingerprint, second.header_fingerprint);
        assert_eq!(first.target, second.target);
        assert_ne!(first, second);
        let state = PoolState {
            idle: VecDeque::new(),
            live_response_ids: Vec::new(),
            settings: ResponsesWebSocketSettings::default(),
            active_api_keys: HashSet::from([second.api_key_id]),
            active_channels: Some(HashMap::from([(
                second.channel_id,
                (
                    second.connector_kind,
                    Arc::clone(&second.connectivity_fingerprint),
                    second.outbound_network_policy_fingerprint,
                    second.plugin_generation.clone(),
                ),
            )])),
        };
        assert!(state.permits(&second));
        assert!(!state.permits(&first));
    }

    #[tokio::test]
    async fn a_connection_returned_after_credential_invalidation_is_discarded() {
        let pool = UpstreamWebSocketPool::new();
        pool.configure(ResponsesWebSocketSettings::new(
            true,
            4,
            Duration::from_secs(60),
            Duration::from_secs(120),
        ));
        let old = key(Uuid::from_u128(4), Uuid::from_u128(5));
        let current = key(Uuid::from_u128(4), Uuid::from_u128(6));
        {
            let mut state = pool.inner.state.lock().unwrap();
            state.active_api_keys.insert(current.api_key_id);
            state.active_channels = Some(HashMap::from([(
                current.channel_id,
                (
                    current.connector_kind,
                    Arc::clone(&current.connectivity_fingerprint),
                    current.outbound_network_policy_fingerprint,
                    current.plugin_generation.clone(),
                ),
            )]));
        }
        let (connection, _incoming) = socket();
        pool.record_connected();
        pool.release(old, connection);
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.leased_connections, 0);
        assert_eq!(snapshot.idle_connections, 0);
        assert_eq!(snapshot.discarded_total, 1);
        assert!(pool.acquire(&current).is_none());
    }

    fn socket() -> (
        UpstreamWebSocket,
        mpsc::Sender<Result<Message, UpstreamWebSocketError>>,
    ) {
        let (commands, _) = mpsc::channel(1);
        let (incoming, messages) = mpsc::channel(1);
        (
            UpstreamWebSocket {
                detached_interrupts: Vec::new(),
                request_limit: None,
                commands,
                messages,
                pump: tokio::spawn(std::future::pending()),
                closed: Arc::new(AtomicBool::new(false)),
                created_at: Instant::now(),
                response_claim: None,
            },
            incoming,
        )
    }

    #[tokio::test]
    async fn late_interrupt_ack_racing_pool_checkout_is_consumed_but_unrelated_tail_is_not_clean() {
        let (mut leased, incoming) = socket();
        let digest = response_id_digest(Some("interrupted")).unwrap();
        assert!(leased.detach_interrupt(digest));
        let metadata = Message::text(r#"{"type":"response.metadata","response_id":"interrupted"}"#);
        let ack =
            Message::text(r#"{"type":"response.interrupt.accepted","response_id":"interrupted"}"#);
        for message in [&metadata, &ack] {
            incoming.try_send(Ok(message.clone())).unwrap();
            assert!(leased.is_clean());
        }
        assert!(!leased.has_detached_interrupt(Some(digest)));
        assert!(leased.detach_interrupt(digest));
        incoming
            .try_send(Ok(Message::text(
                r#"{"type":"response.interrupt.accepted","response_id":"foreign"}"#,
            )))
            .unwrap();
        assert!(!leased.is_clean());
    }

    #[tokio::test]
    async fn detached_interrupts_do_not_overwrite_an_older_identity() {
        let (mut leased, _incoming) = socket();
        for i in 0..MAX_DETACHED_INTERRUPTS {
            assert!(leased.detach_interrupt([i as u8; 32]));
        }
        assert!(!leased.detach_interrupt([255; 32]));
        assert!(leased.discard_detached_control(InterruptControl::Accepted, Some([0; 32])));
        assert!(leased.detach_interrupt([255; 32]));
    }

    #[tokio::test]
    async fn colliding_completion_while_original_is_leased_invalidates_both_continuations() {
        let pool = UpstreamWebSocketPool::new();
        pool.configure(ResponsesWebSocketSettings::new(
            true,
            4,
            Duration::from_secs(60),
            Duration::from_secs(120),
        ));
        let first_key = key(Uuid::from_u128(4), Uuid::from_u128(5));
        let mut second_key = first_key.clone();
        second_key.channel_id = Uuid::from_u128(6);
        let digest = [42; 32];
        let (mut first, _incoming_a) = socket();
        pool.record_connected();
        pool.observe_completion(&first_key, &mut first, Some(digest));
        pool.release(first_key.clone(), first);
        let (leased_key, mut leased) = pool
            .acquire_continuation(first_key.api_key_id, first_key.client_identity, digest)
            .unwrap();
        assert!(pool.continuation_is_ready(&leased_key, &mut leased));
        let (mut second, _incoming_b) = socket();
        pool.record_connected();
        pool.observe_completion(&second_key, &mut second, Some(digest));
        pool.release(second_key, second);
        assert!(!pool.continuation_is_ready(&leased_key, &mut leased));
        assert!(
            pool.acquire_continuation(first_key.api_key_id, first_key.client_identity, digest)
                .is_none()
        );
        drop(leased);
        pool.discard_leased();
        assert!(
            pool.acquire_continuation(first_key.api_key_id, first_key.client_identity, digest)
                .is_none()
        );
        assert_eq!(pool.snapshot().leased_connections, 0);
    }

    #[tokio::test]
    async fn consecutive_completions_with_the_same_id_are_not_distinguishable() {
        let pool = UpstreamWebSocketPool::new();
        pool.configure(ResponsesWebSocketSettings::new(
            true,
            4,
            Duration::from_secs(60),
            Duration::from_secs(120),
        ));
        let key = key(Uuid::from_u128(4), Uuid::from_u128(5));
        let (mut socket, _incoming) = socket();
        pool.record_connected();
        let id = [42; 32];
        pool.observe_completion(&key, &mut socket, Some(id));
        pool.release(key.clone(), socket);
        let (_, mut socket) = pool
            .acquire_continuation(key.api_key_id, key.client_identity, id)
            .unwrap();
        pool.observe_completion(&key, &mut socket, Some(id));
        pool.release(key.clone(), socket);
        assert!(
            pool.acquire_continuation(key.api_key_id, key.client_identity, id)
                .is_none()
        );
    }

    #[tokio::test]
    async fn late_residual_frame_or_max_age_invalidates_a_reserved_continuation() {
        let pool = UpstreamWebSocketPool::new();
        pool.configure(ResponsesWebSocketSettings::new(
            true,
            4,
            Duration::from_secs(60),
            Duration::from_secs(120),
        ));
        let key = key(Uuid::from_u128(4), Uuid::from_u128(5));
        for expired in [false, true] {
            let (mut connection, incoming) = socket();
            pool.record_connected();
            pool.observe_completion(&key, &mut connection, Some([42; 32]));
            pool.release(key.clone(), connection);
            let (_, mut leased) = pool
                .acquire_continuation(key.api_key_id, key.client_identity, [42; 32])
                .unwrap();
            if expired {
                leased.created_at -= Duration::from_secs(120);
            } else {
                incoming
                    .try_send(Ok(Message::Text("residual".into())))
                    .unwrap();
            }
            assert!(!pool.continuation_is_ready(&key, &mut leased));
            pool.discard_leased();
        }
        assert_eq!(pool.snapshot().leased_connections, 0);
    }
}

fn hash_value(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

/// Safe, credential-free WebSocket setup or I/O failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum UpstreamWebSocketError {
    #[error("invalid upstream websocket configuration")]
    InvalidConfiguration,
    #[error("upstream websocket connection timed out")]
    ConnectTimeout,
    #[error("upstream websocket handshake timed out")]
    HandshakeTimeout,
    #[error("upstream websocket network failure")]
    Network,
    #[error("upstream websocket handshake returned HTTP {status}")]
    Http { status: u16 },
    #[error("upstream websocket is closed")]
    Closed,
}

enum WebSocketCommand {
    Send {
        message: Message,
        result: oneshot::Sender<Result<(), UpstreamWebSocketError>>,
    },
}

/// One exclusively leased upstream WebSocket backed by a control-frame pump.
pub(crate) struct UpstreamWebSocket {
    /// Completed turns whose controls must not enter a subsequent logical request.
    detached_interrupts: Vec<[u8; 32]>,
    request_limit: Option<usize>,
    commands: mpsc::Sender<WebSocketCommand>,
    messages: mpsc::Receiver<Result<Message, UpstreamWebSocketError>>,
    pump: tokio::task::JoinHandle<()>,
    closed: Arc<AtomicBool>,
    created_at: Instant,
    response_claim: Option<Arc<ResponseClaim>>,
}

const MAX_DETACHED_INTERRUPTS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterruptControl {
    Accepted,
    Rejected,
    Metadata,
}

impl InterruptControl {
    pub(crate) fn from_kind(kind: Option<&str>) -> Option<Self> {
        match kind? {
            "response.interrupt.accepted" => Some(Self::Accepted),
            "response.interrupt.failed" => Some(Self::Rejected),
            "response.metadata"
            | "codex.response.metadata"
            | "codex.rate_limits"
            | "responsesapi.websocket_timing" => Some(Self::Metadata),
            _ => None,
        }
    }

    pub(crate) fn matches(
        self,
        id: Option<[u8; 32]>,
        expected: [u8; 32],
        before_response: bool,
    ) -> bool {
        id == Some(expected) || self == Self::Metadata && before_response && id.is_none()
    }
}

pub(crate) fn response_id_digest(id: Option<&str>) -> Option<[u8; 32]> {
    id.filter(|id| !id.is_empty())
        .map(|id| Sha256::digest(id.as_bytes()).into())
}

impl UpstreamWebSocket {
    fn discard_queued_interrupt_control(&mut self, message: Message) -> bool {
        // Pool checkout holds a process-wide lock. Fail closed on oversized or noisy tails.
        let Message::Text(text) = &message else {
            return false;
        };
        if self.detached_interrupts.is_empty() || text.len() > 4096 {
            return false;
        }
        #[derive(serde::Deserialize)]
        struct Probe<'a> {
            #[serde(rename = "type")]
            kind: &'a str,
            #[serde(borrow)]
            response_id: Option<std::borrow::Cow<'a, str>>,
        }
        let Ok(probe) = serde_json::from_str::<Probe<'_>>(text) else {
            return false;
        };
        let Some(control) = InterruptControl::from_kind(Some(probe.kind)) else {
            return false;
        };
        self.discard_detached_control(control, response_id_digest(probe.response_id.as_deref()))
    }
}

impl UpstreamWebSocket {
    fn new(stream: dialer::UpstreamStream, request_limit: Option<usize>) -> Self {
        let (commands, mut command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (message_tx, messages) = mpsc::channel(MESSAGE_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let pump_closed = Arc::clone(&closed);
        let pump = tokio::spawn(async move {
            let mut stream = stream;
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        match command {
                            WebSocketCommand::Send { message, result } => {
                                let sent = stream
                                    .send(message)
                                    .await
                                    .map_err(|_| UpstreamWebSocketError::Network);
                                let should_close = sent.is_err();
                                let _ = result.send(sent);
                                if should_close {
                                    break;
                                }
                            }
                        }
                    }
                    incoming = stream.next() => {
                        let Some(incoming) = incoming else {
                            break;
                        };
                        match incoming {
                            Ok(Message::Ping(payload)) => {
                                if stream.send(Message::Pong(payload)).await.is_err() {
                                    let _ = message_tx.send(Err(UpstreamWebSocketError::Network)).await;
                                    break;
                                }
                            }
                            Ok(Message::Pong(_)) => {}
                            Ok(message @ (Message::Text(_)
                                | Message::Binary(_)
                                | Message::Close(_))) => {
                                let is_close = matches!(message, Message::Close(_));
                                if message_tx.send(Ok(message)).await.is_err() {
                                    break;
                                }
                                if is_close {
                                    break;
                                }
                            }
                            Ok(Message::Frame(_)) => {}
                            Err(_) => {
                                let _ = message_tx.send(Err(UpstreamWebSocketError::Network)).await;
                                break;
                            }
                        }
                    }
                }
            }
            pump_closed.store(true, Ordering::Release);
        });
        Self {
            detached_interrupts: Vec::new(),
            request_limit,
            commands,
            messages,
            pump,
            closed,
            created_at: Instant::now(),
            response_claim: None,
        }
    }

    pub fn request_too_large(&self, bytes: usize) -> bool {
        self.request_limit.is_some_and(|limit| bytes > limit)
    }

    pub async fn send(&self, message: Message) -> Result<(), UpstreamWebSocketError> {
        let (result_tx, result_rx) = oneshot::channel();
        self.commands
            .send(WebSocketCommand::Send {
                message,
                result: result_tx,
            })
            .await
            .map_err(|_| UpstreamWebSocketError::Closed)?;
        result_rx
            .await
            .unwrap_or(Err(UpstreamWebSocketError::Closed))
    }

    pub async fn next(&mut self) -> Option<Result<Message, UpstreamWebSocketError>> {
        self.messages.recv().await
    }

    pub(crate) fn detach_interrupt(&mut self, response_id: [u8; 32]) -> bool {
        if self.detached_interrupts.len() == MAX_DETACHED_INTERRUPTS
            || self.detached_interrupts.contains(&response_id)
        {
            return false;
        }
        self.detached_interrupts.push(response_id);
        true
    }

    pub(crate) fn has_detached_interrupt(&self, id: Option<[u8; 32]>) -> bool {
        id.is_some_and(|id| self.detached_interrupts.contains(&id))
    }

    pub(crate) fn discard_detached_control(
        &mut self,
        control: InterruptControl,
        id: Option<[u8; 32]>,
    ) -> bool {
        let Some(id) = id else {
            // Unscoped metadata cannot be attributed to this new turn while an older tail is live.
            return control == InterruptControl::Metadata && !self.detached_interrupts.is_empty();
        };
        let Some(index) = self.detached_interrupts.iter().position(|old| *old == id) else {
            return false;
        };
        if control != InterruptControl::Metadata {
            self.detached_interrupts.swap_remove(index);
        }
        true
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn is_clean(&mut self) -> bool {
        if self.is_closed() {
            return false;
        }
        for consumed in 0..=4 {
            match self.messages.try_recv() {
                Err(mpsc::error::TryRecvError::Empty) => return true,
                Ok(Ok(message)) => {
                    if consumed == 4 || !self.discard_queued_interrupt_control(message) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        false
    }

    fn age(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.created_at)
    }
}

impl Drop for UpstreamWebSocket {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// Opens one upstream WebSocket using the channel's proxy, TLS, and timeout policy.
pub(crate) async fn connect_upstream_websocket(
    target: Url,
    headers: HeaderMap,
    channel: &CompiledChannelUpstreamPolicy,
    policy: ResolvedUpstreamPolicy,
    max_message_bytes: usize,
) -> Result<UpstreamWebSocket, UpstreamWebSocketError> {
    let mut config = WebSocketConfig::default()
        .read_buffer_size(64 * 1024)
        .write_buffer_size(32 * 1024)
        .max_write_buffer_size(max_message_bytes.saturating_add(32 * 1024))
        .max_message_size(Some(max_message_bytes))
        .max_frame_size(Some(max_message_bytes));
    config.extensions.permessage_deflate = Some(DeflateConfig::default());
    let (connected, request_limit) = tokio::time::timeout(
        policy.timeouts().response_header(),
        dialer::connect(
            target,
            headers,
            channel,
            policy.timeouts().connect(),
            config,
        ),
    )
    .await
    .map_err(|_| UpstreamWebSocketError::HandshakeTimeout)??;
    Ok(UpstreamWebSocket::new(connected, request_limit))
}

struct IdleWebSocket {
    key: UpstreamWebSocketKey,
    connection: UpstreamWebSocket,
    idle_since: Instant,
}

struct ResponseClaim {
    api_key_id: Uuid,
    client_identity: WebSocketClientIdentity,
    digest: [u8; 32],
    ambiguous: AtomicBool,
}

impl ResponseClaim {
    fn matches(
        &self,
        api_key_id: Uuid,
        client_identity: WebSocketClientIdentity,
        digest: [u8; 32],
    ) -> bool {
        self.api_key_id == api_key_id
            && self.client_identity == client_identity
            && self.digest == digest
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WebSocketPoolSnapshot {
    pub(crate) enabled: bool,
    pub(crate) idle_connections: u64,
    pub(crate) leased_connections: u64,
    pub(crate) capacity: u64,
    pub(crate) hits_total: u64,
    pub(crate) misses_total: u64,
    pub(crate) discarded_total: u64,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) max_connection_age_seconds: u64,
}

pub(super) struct UpstreamWebSocketPool {
    inner: Arc<PoolInner>,
    reaper_started: AtomicBool,
}

struct PoolInner {
    state: Mutex<PoolState>,
    leased_connections: AtomicU64,
    hits_total: AtomicU64,
    misses_total: AtomicU64,
    discarded_total: AtomicU64,
}

type ActiveChannelIdentity = (
    ConnectorKind,
    Arc<str>,
    OutboundNetworkPolicyFingerprint,
    Option<Arc<str>>,
);

struct PoolState {
    idle: VecDeque<IdleWebSocket>,
    // Borrowed sockets keep their claims until the next terminal response or discard.
    live_response_ids: Vec<Weak<ResponseClaim>>,
    settings: ResponsesWebSocketSettings,
    active_channels: Option<HashMap<Uuid, ActiveChannelIdentity>>,
    active_api_keys: HashSet<Uuid>,
}

impl PoolState {
    fn permits(&self, key: &UpstreamWebSocketKey) -> bool {
        self.active_channels.as_ref().is_none_or(|channels| {
            self.active_api_keys.contains(&key.api_key_id)
                && channels.get(&key.channel_id).is_some_and(
                    |(connector, identity, network, generation)| {
                        connector == &key.connector_kind
                            && identity == &key.connectivity_fingerprint
                            && network == &key.outbound_network_policy_fingerprint
                            && generation == &key.plugin_generation
                    },
                )
        })
    }
}

impl UpstreamWebSocketPool {
    #[must_use]
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(PoolInner {
                state: Mutex::new(PoolState {
                    idle: VecDeque::new(),
                    live_response_ids: Vec::new(),
                    settings: ResponsesWebSocketSettings::default(),
                    active_channels: None,
                    active_api_keys: HashSet::new(),
                }),
                leased_connections: AtomicU64::new(0),
                hits_total: AtomicU64::new(0),
                misses_total: AtomicU64::new(0),
                discarded_total: AtomicU64::new(0),
            }),
            reaper_started: AtomicBool::new(false),
        }
    }

    pub(super) fn configure(&self, settings: ResponsesWebSocketSettings) {
        let now = Instant::now();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        state.settings = settings;
        let mut discarded = prune_idle(&mut state, now);
        if !settings.enabled() {
            discarded = discarded.saturating_add(clear_idle(&mut state));
        } else {
            discarded = discarded.saturating_add(trim_to_capacity(&mut state));
        }
        drop(state);
        record_discarded(&self.inner, discarded);
    }

    pub(super) fn acquire(&self, key: &UpstreamWebSocketKey) -> Option<UpstreamWebSocket> {
        self.checkout(|idle| idle.iter().rposition(|entry| entry.key == *key))
            .map(|entry| entry.connection)
    }

    pub(super) fn acquire_continuation(
        &self,
        api_key_id: Uuid,
        client_identity: WebSocketClientIdentity,
        response_id: [u8; 32],
    ) -> Option<(UpstreamWebSocketKey, UpstreamWebSocket)> {
        self.checkout(|idle| {
            idle.iter().position(|entry| {
                entry
                    .connection
                    .response_claim
                    .as_ref()
                    .is_some_and(|claim| {
                        claim.matches(api_key_id, client_identity, response_id)
                            && !claim.ambiguous.load(Ordering::Relaxed)
                    })
            })
        })
        .map(|entry| (entry.key, entry.connection))
    }

    pub(super) fn continuation_is_ready(
        &self,
        key: &UpstreamWebSocketKey,
        connection: &mut UpstreamWebSocket,
    ) -> bool {
        let state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        state.settings.enabled()
            && state.permits(key)
            && connection.age(Instant::now()) < state.settings.max_connection_age()
            && connection
                .response_claim
                .as_ref()
                .is_some_and(|claim| !claim.ambiguous.load(Ordering::Relaxed))
            && connection.is_clean()
    }

    fn checkout(
        &self,
        select: impl FnOnce(&VecDeque<IdleWebSocket>) -> Option<usize>,
    ) -> Option<IdleWebSocket> {
        let now = Instant::now();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        let discarded = prune_idle(&mut state, now);
        record_discarded(&self.inner, discarded);
        let index = select(&state.idle)
            .filter(|index| state.settings.enabled() && state.permits(&state.idle[*index].key));
        let Some(index) = index else {
            self.inner.misses_total.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let entry = state
            .idle
            .remove(index)
            .expect("matched websocket pool entry exists");
        self.inner.hits_total.fetch_add(1, Ordering::Relaxed);
        self.inner.leased_connections.fetch_add(1, Ordering::AcqRel);
        Some(entry)
    }

    pub(super) fn observe_completion(
        &self,
        key: &UpstreamWebSocketKey,
        connection: &mut UpstreamWebSocket,
        last_response_id: Option<[u8; 32]>,
    ) {
        let Some(digest) = last_response_id else {
            connection.response_claim = None;
            return;
        };
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        let mut collision = None;
        state.live_response_ids.retain(|weak| {
            let Some(claim) = weak.upgrade() else {
                return false;
            };
            if claim.matches(key.api_key_id, key.client_identity, digest) {
                claim.ambiguous.store(true, Ordering::Relaxed);
                collision = Some(claim);
            }
            true
        });
        connection.response_claim = Some(collision.unwrap_or_else(|| {
            let claim = Arc::new(ResponseClaim {
                api_key_id: key.api_key_id,
                client_identity: key.client_identity,
                digest,
                ambiguous: AtomicBool::new(false),
            });
            state.live_response_ids.push(Arc::downgrade(&claim));
            claim
        }));
    }

    pub(super) fn release(&self, key: UpstreamWebSocketKey, mut connection: UpstreamWebSocket) {
        decrement(&self.inner.leased_connections);
        let now = Instant::now();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        let settings = state.settings;
        let mut discarded = prune_idle(&mut state, now);
        if !settings.enabled()
            || !state.permits(&key)
            || settings.max_idle_connections() == 0
            || connection.age(now) >= settings.max_connection_age()
            || !connection.is_clean()
        {
            drop(state);
            record_discarded(&self.inner, discarded.saturating_add(1));
            return;
        }
        self.ensure_reaper();
        let before = state.idle.len();
        state.idle.retain(|entry| entry.key != key);
        discarded = discarded.saturating_add(usize_to_u64(before - state.idle.len()));
        state.idle.push_back(IdleWebSocket {
            key,
            connection,
            idle_since: now,
        });
        discarded = discarded.saturating_add(trim_to_capacity(&mut state));
        drop(state);
        record_discarded(&self.inner, discarded);
    }

    pub(super) fn record_connected(&self) {
        self.inner.leased_connections.fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn discard_leased(&self) {
        decrement(&self.inner.leased_connections);
        self.inner.discarded_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn reconcile(&self, snapshot: &CompiledRuntimeConfig) {
        let settings = snapshot.system_settings().websocket();
        let active_api_keys = snapshot
            .api_keys()
            .filter(|api_key| api_key.websocket_enabled())
            .map(|api_key| api_key.id())
            .collect::<HashSet<_>>();
        let active_channels = snapshot
            .model_rules()
            .flat_map(|rule| rule.tiers())
            .flat_map(crate::domain::CompiledRouteTier::candidates)
            .filter(|candidate| candidate.channel().supports_websocket())
            .map(|candidate| candidate.channel().id())
            .collect::<HashSet<_>>();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        state.settings = settings;
        state.active_api_keys = active_api_keys.clone();
        state.active_channels = Some(
            active_channels
                .iter()
                .filter_map(|id| {
                    snapshot.channel(*id).map(|channel| {
                        (
                            *id,
                            (
                                channel.connector_kind(),
                                Arc::clone(channel.connectivity_fingerprint()),
                                channel
                                    .upstream_policy()
                                    .outbound_network_policy_fingerprint(),
                                snapshot
                                    .plugins()
                                    .get(channel.connector_kind().as_str())
                                    .map(|plugin| Arc::from(plugin.generation_id())),
                            ),
                        )
                    })
                })
                .collect(),
        );
        let mut discarded = prune_idle(&mut state, Instant::now());
        let before = state.idle.len();
        state.idle.retain(|entry| {
            settings.enabled()
                && active_api_keys.contains(&entry.key.api_key_id)
                && active_channels.contains(&entry.key.channel_id)
                && snapshot
                    .channel(entry.key.channel_id)
                    .is_some_and(|channel| {
                        channel.supports_websocket()
                            && channel.connector_kind() == entry.key.connector_kind
                            && snapshot
                                .plugins()
                                .get(channel.connector_kind().as_str())
                                .map(|plugin| Arc::<str>::from(plugin.generation_id()))
                                == entry.key.plugin_generation
                            && channel.connectivity_fingerprint()
                                == &entry.key.connectivity_fingerprint
                            && channel
                                .upstream_policy()
                                .outbound_network_policy_fingerprint()
                                == entry.key.outbound_network_policy_fingerprint
                    })
        });
        discarded = discarded.saturating_add(usize_to_u64(before - state.idle.len()));
        discarded = discarded.saturating_add(trim_to_capacity(&mut state));
        drop(state);
        record_discarded(&self.inner, discarded);
    }

    pub(super) fn snapshot(&self) -> WebSocketPoolSnapshot {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        let discarded = prune_idle(&mut state, Instant::now());
        record_discarded(&self.inner, discarded);
        let settings = state.settings;
        WebSocketPoolSnapshot {
            enabled: settings.enabled(),
            idle_connections: usize_to_u64(state.idle.len()),
            leased_connections: self.inner.leased_connections.load(Ordering::Acquire),
            capacity: usize_to_u64(settings.max_idle_connections()),
            hits_total: self.inner.hits_total.load(Ordering::Relaxed),
            misses_total: self.inner.misses_total.load(Ordering::Relaxed),
            discarded_total: self.inner.discarded_total.load(Ordering::Relaxed),
            idle_timeout_seconds: settings.idle_timeout().as_secs(),
            max_connection_age_seconds: settings.max_connection_age().as_secs(),
        }
    }

    pub(super) fn clear(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("websocket pool mutex poisoned");
        let discarded = clear_idle(&mut state);
        drop(state);
        record_discarded(&self.inner, discarded);
    }

    fn ensure_reaper(&self) {
        if self.reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let inner = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAPER_INTERVAL).await;
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                let mut state = inner.state.lock().expect("websocket pool mutex poisoned");
                let mut discarded = prune_idle(&mut state, Instant::now());
                discarded = discarded.saturating_add(trim_to_capacity(&mut state));
                drop(state);
                record_discarded(&inner, discarded);
            }
        });
    }
}

fn prune_idle(state: &mut PoolState, now: Instant) -> u64 {
    let before = state.idle.len();
    let idle_timeout = state.settings.idle_timeout();
    let max_connection_age = state.settings.max_connection_age();
    state.idle.retain_mut(|entry| {
        entry.connection.is_clean()
            && now.saturating_duration_since(entry.idle_since) < idle_timeout
            && entry.connection.age(now) < max_connection_age
    });
    usize_to_u64(before - state.idle.len())
}

fn trim_to_capacity(state: &mut PoolState) -> u64 {
    let mut discarded = 0_u64;
    while state.idle.len() > state.settings.max_idle_connections() {
        state.idle.pop_front();
        discarded = discarded.saturating_add(1);
    }
    discarded
}

fn clear_idle(state: &mut PoolState) -> u64 {
    let discarded = usize_to_u64(state.idle.len());
    state.idle.clear();
    discarded
}

fn record_discarded(inner: &PoolInner, count: u64) {
    if count > 0 {
        inner.discarded_total.fetch_add(count, Ordering::Relaxed);
    }
}

fn decrement(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        Some(value.saturating_sub(1))
    });
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
