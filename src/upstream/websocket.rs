//! Bounded, connection-local Responses WebSocket reuse and message pumping.

mod dialer;

use std::{
    collections::{HashSet, VecDeque},
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, Uri};
use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Message, protocol::WebSocketConfig};
use uuid::Uuid;

use crate::domain::{
    CompiledChannel, CompiledChannelUpstreamPolicy, CompiledRuntimeConfig,
    OutboundNetworkPolicyFingerprint,
};

use super::ResolvedUpstreamPolicy;

const COMMAND_CAPACITY: usize = 8;
const MESSAGE_CAPACITY: usize = 16;
const IDLE_POOL_CAPACITY: usize = 128;
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_CONNECTION_AGE: Duration = Duration::from_secs(55 * 60);
const REAPER_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const MAX_UPSTREAM_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

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
    pub(crate) fn channel_id(&self) -> Uuid {
        self.channel_id
    }
}

impl fmt::Debug for UpstreamWebSocketKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamWebSocketKey")
            .field("api_key_id", &self.api_key_id)
            .field("client_identity", &self.client_identity)
            .field("channel_id", &self.channel_id)
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
    commands: mpsc::Sender<WebSocketCommand>,
    messages: mpsc::Receiver<Result<Message, UpstreamWebSocketError>>,
    pump: tokio::task::JoinHandle<()>,
    closed: Arc<AtomicBool>,
    created_at: Instant,
}

impl UpstreamWebSocket {
    fn new(stream: dialer::UpstreamStream) -> Self {
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
            commands,
            messages,
            pump,
            closed,
            created_at: Instant::now(),
        }
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

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn is_clean(&mut self) -> bool {
        !self.is_closed()
            && matches!(
                self.messages.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            )
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
    let config = WebSocketConfig::default()
        .read_buffer_size(64 * 1024)
        .write_buffer_size(32 * 1024)
        .max_write_buffer_size(max_message_bytes.saturating_add(32 * 1024))
        .max_message_size(Some(max_message_bytes))
        .max_frame_size(Some(max_message_bytes));
    let connected = tokio::time::timeout(
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
    Ok(UpstreamWebSocket::new(connected))
}

struct IdleWebSocket {
    key: UpstreamWebSocketKey,
    connection: UpstreamWebSocket,
    idle_since: Instant,
}

pub(super) struct UpstreamWebSocketPool {
    inner: Arc<PoolInner>,
    reaper_started: AtomicBool,
}

struct PoolInner {
    idle: Mutex<VecDeque<IdleWebSocket>>,
}

impl UpstreamWebSocketPool {
    #[must_use]
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(PoolInner {
                idle: Mutex::new(VecDeque::new()),
            }),
            reaper_started: AtomicBool::new(false),
        }
    }

    pub(super) fn acquire(&self, key: &UpstreamWebSocketKey) -> Option<UpstreamWebSocket> {
        let now = Instant::now();
        let mut idle = self
            .inner
            .idle
            .lock()
            .expect("websocket pool mutex poisoned");
        prune_idle(&mut idle, now);
        let index = idle.iter().rposition(|entry| entry.key == *key)?;
        idle.remove(index).map(|entry| entry.connection)
    }

    pub(super) fn release(&self, key: UpstreamWebSocketKey, mut connection: UpstreamWebSocket) {
        let now = Instant::now();
        if connection.age(now) >= MAX_CONNECTION_AGE || !connection.is_clean() {
            return;
        }
        self.ensure_reaper();
        let mut idle = self
            .inner
            .idle
            .lock()
            .expect("websocket pool mutex poisoned");
        prune_idle(&mut idle, now);
        idle.retain(|entry| entry.key != key);
        idle.push_back(IdleWebSocket {
            key,
            connection,
            idle_since: now,
        });
        while idle.len() > IDLE_POOL_CAPACITY {
            idle.pop_front();
        }
    }

    pub(super) fn preferred_channel(
        &self,
        api_key_id: Uuid,
        client_identity: WebSocketClientIdentity,
    ) -> Option<Uuid> {
        let now = Instant::now();
        let mut idle = self
            .inner
            .idle
            .lock()
            .expect("websocket pool mutex poisoned");
        prune_idle(&mut idle, now);
        idle.iter()
            .rev()
            .find(|entry| {
                entry.key.api_key_id == api_key_id && entry.key.client_identity == client_identity
            })
            .map(|entry| entry.key.channel_id)
    }

    pub(super) fn reconcile(&self, snapshot: &CompiledRuntimeConfig) {
        let active_api_keys = snapshot
            .api_keys()
            .map(|api_key| api_key.id())
            .collect::<HashSet<_>>();
        let active_channels = snapshot
            .model_rules()
            .flat_map(|rule| rule.tiers())
            .flat_map(crate::domain::CompiledRouteTier::candidates)
            .map(|candidate| candidate.channel().id())
            .collect::<HashSet<_>>();
        let mut idle = self
            .inner
            .idle
            .lock()
            .expect("websocket pool mutex poisoned");
        prune_idle(&mut idle, Instant::now());
        idle.retain(|entry| {
            active_api_keys.contains(&entry.key.api_key_id)
                && active_channels.contains(&entry.key.channel_id)
                && snapshot
                    .channel(entry.key.channel_id)
                    .is_some_and(|channel| {
                        channel.connectivity_fingerprint() == &entry.key.connectivity_fingerprint
                            && channel
                                .upstream_policy()
                                .outbound_network_policy_fingerprint()
                                == entry.key.outbound_network_policy_fingerprint
                    })
        });
    }

    pub(super) fn clear(&self) {
        self.inner
            .idle
            .lock()
            .expect("websocket pool mutex poisoned")
            .clear();
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
                let mut idle = inner.idle.lock().expect("websocket pool mutex poisoned");
                prune_idle(&mut idle, Instant::now());
            }
        });
    }
}

fn prune_idle(idle: &mut VecDeque<IdleWebSocket>, now: Instant) {
    idle.retain_mut(|entry| {
        entry.connection.is_clean()
            && now.saturating_duration_since(entry.idle_since) < IDLE_TIMEOUT
            && entry.connection.age(now) < MAX_CONNECTION_AGE
    });
}
