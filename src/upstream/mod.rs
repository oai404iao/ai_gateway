//! Reusable reqwest clients keyed by compiled outbound network policy.

mod websocket;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    sync::Mutex,
    time::Duration,
};

use reqwest::{Client, Proxy, Url, redirect::Policy};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::domain::{
    CompiledChannelUpstreamPolicy, CompiledRuntimeConfig, NoProxyHost, ResponsesWebSocketSettings,
    UpstreamTimeoutDefaults,
};

pub(crate) use websocket::{
    MAX_UPSTREAM_MESSAGE_BYTES, UpstreamWebSocket, UpstreamWebSocketError, UpstreamWebSocketKey,
    WebSocketClientIdentity, WebSocketPoolSnapshot, connect_upstream_websocket,
};

/// The only TLS configuration permitted for upstream clients.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UpstreamTlsPolicy {
    /// Rustls with the bundled WebPKI root store.
    RustlsWebPkiRoots,
}

/// Effective upstream timeouts after applying channel overrides to database
/// system-setting defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedUpstreamTimeouts {
    connect: Duration,
    response_header: Duration,
    stream_idle: Duration,
}

impl ResolvedUpstreamTimeouts {
    /// Merges positive, compiled channel values over global system defaults.
    #[must_use]
    pub fn resolve(
        upstream: &UpstreamTimeoutDefaults,
        channel: &CompiledChannelUpstreamPolicy,
    ) -> Self {
        let overrides = channel.timeouts();
        Self {
            connect: overrides.connect().unwrap_or_else(|| upstream.connect()),
            response_header: overrides
                .response_header()
                .unwrap_or_else(|| upstream.response_header()),
            stream_idle: overrides
                .stream_idle()
                .unwrap_or_else(|| upstream.stream_idle()),
        }
    }

    #[must_use]
    pub fn connect(self) -> Duration {
        self.connect
    }

    /// Applied by the request lifecycle, not by the reqwest client.
    #[must_use]
    pub fn response_header(self) -> Duration {
        self.response_header
    }

    /// Applied while consuming the upstream response stream, not by reqwest.
    #[must_use]
    pub fn stream_idle(self) -> Duration {
        self.stream_idle
    }
}

/// Effective client and request-lifecycle policy for a selected channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedUpstreamPolicy {
    timeouts: ResolvedUpstreamTimeouts,
    tls: UpstreamTlsPolicy,
}

impl ResolvedUpstreamPolicy {
    #[must_use]
    pub fn resolve(
        upstream: &UpstreamTimeoutDefaults,
        channel: &CompiledChannelUpstreamPolicy,
    ) -> Self {
        Self {
            timeouts: ResolvedUpstreamTimeouts::resolve(upstream, channel),
            tls: UpstreamTlsPolicy::RustlsWebPkiRoots,
        }
    }

    /// Resolves and validates the effective policy before it is used for any
    /// outbound request work.
    pub fn try_resolve(
        upstream: &UpstreamTimeoutDefaults,
        channel: &CompiledChannelUpstreamPolicy,
    ) -> Result<Self, ResolvedUpstreamPolicyError> {
        Self::resolve(upstream, channel).validate()
    }

    /// Requires enough time for a connection attempt to complete before the
    /// response-header deadline expires.
    pub fn validate(self) -> Result<Self, ResolvedUpstreamPolicyError> {
        (self.timeouts.response_header() > self.timeouts.connect())
            .then_some(self)
            .ok_or(ResolvedUpstreamPolicyError::InvalidTimeoutOrdering)
    }

    #[must_use]
    pub fn timeouts(self) -> ResolvedUpstreamTimeouts {
        self.timeouts
    }

    #[must_use]
    pub fn tls(self) -> UpstreamTlsPolicy {
        self.tls
    }
}

/// Safe, value-free error for an invalid effective outbound policy.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ResolvedUpstreamPolicyError {
    #[error("invalid resolved upstream timeout policy")]
    InvalidTimeoutOrdering,
}

/// Validates every compiled channel against the system defaults carried by a
/// candidate snapshot before it is made active.
///
/// The error is deliberately value-free so it is safe to surface at process
/// and control-plane boundaries.
pub fn validate_snapshot_upstream_policies(
    snapshot: &CompiledRuntimeConfig,
) -> Result<(), ResolvedUpstreamPolicyError> {
    let upstream_defaults = snapshot.system_settings().upstream_timeouts();
    snapshot.probe_channels().try_for_each(|channel| {
        ResolvedUpstreamPolicy::try_resolve(&upstream_defaults, channel.upstream_policy())
            .map(|_| ())
    })
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum ClientProxyKey {
    Direct,
    Configured {
        base_url: Box<str>,
        credential_fingerprint: [u8; 32],
        no_proxy_hosts: Box<[Box<str>]>,
    },
}

/// Credential-free, deterministic key for a reusable reqwest client.
///
/// Its deliberately sparse debug and display implementations make it safe to
/// include in operational errors without revealing proxy credentials.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct UpstreamClientKey {
    proxy: ClientProxyKey,
    connect_timeout: Duration,
    tls: UpstreamTlsPolicy,
}

impl UpstreamClientKey {
    #[must_use]
    pub fn resolve(
        channel: &CompiledChannelUpstreamPolicy,
        policy: ResolvedUpstreamPolicy,
    ) -> Self {
        let proxy = channel.proxy().map_or(ClientProxyKey::Direct, |proxy| {
            let mut no_proxy_hosts = proxy
                .no_proxy_hosts()
                .iter()
                .map(normalized_no_proxy_host)
                .collect::<Vec<_>>();
            no_proxy_hosts.sort_unstable();
            no_proxy_hosts.dedup();

            ClientProxyKey::Configured {
                base_url: credential_free_proxy_url(proxy.url()).into_boxed_str(),
                credential_fingerprint: credential_fingerprint(proxy.username(), proxy.password()),
                no_proxy_hosts: no_proxy_hosts.into_boxed_slice(),
            }
        });
        Self {
            proxy,
            connect_timeout: policy.timeouts().connect(),
            tls: policy.tls(),
        }
    }

    #[must_use]
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    #[must_use]
    pub fn tls(&self) -> UpstreamTlsPolicy {
        self.tls
    }
}

impl fmt::Debug for UpstreamClientKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpstreamClientKey")
            .field(
                "proxy",
                &match self.proxy {
                    ClientProxyKey::Direct => "direct",
                    ClientProxyKey::Configured { .. } => "configured",
                },
            )
            .field("connect_timeout", &self.connect_timeout)
            .field("tls", &self.tls)
            .finish()
    }
}

impl fmt::Display for UpstreamClientKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("upstream client configuration")
    }
}

/// Safe, opaque failure returned when a reqwest client cannot be constructed.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum UpstreamClientError {
    #[error("unable to build upstream client")]
    Build,
    #[error("upstream client registry is unavailable")]
    RegistryUnavailable,
    #[error("invalid resolved upstream policy")]
    InvalidPolicy,
}

/// Maximum number of distinct outbound client configurations retained per process.
pub const UPSTREAM_CLIENT_REGISTRY_CAPACITY: usize = 64;
/// Maximum number of manual Console diagnostic client configurations retained per process.
pub const DIAGNOSTIC_CLIENT_REGISTRY_CAPACITY: usize = 16;

/// Process-lifetime, bounded LRU cache of reusable reqwest clients.
///
/// Removing a client from this cache only drops the map's clone. A client
/// already cloned by an in-flight request remains valid.
pub struct UpstreamClientRegistry {
    entries: Mutex<RegistryEntries>,
    diagnostic_entries: Mutex<RegistryEntries>,
    websockets: websocket::UpstreamWebSocketPool,
}

struct RegistryEntries {
    clients: HashMap<UpstreamClientKey, Client>,
    least_to_most_recent: VecDeque<UpstreamClientKey>,
    active_keys: Option<HashSet<UpstreamClientKey>>,
}

impl Default for UpstreamClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl UpstreamClientRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(RegistryEntries {
                clients: HashMap::new(),
                least_to_most_recent: VecDeque::new(),
                active_keys: None,
            }),
            diagnostic_entries: Mutex::new(RegistryEntries {
                clients: HashMap::new(),
                least_to_most_recent: VecDeque::new(),
                active_keys: None,
            }),
            websockets: websocket::UpstreamWebSocketPool::new(),
        }
    }

    /// Returns a client for the compiled channel policy and its resolved timeouts.
    pub fn client_for(
        &self,
        channel: &CompiledChannelUpstreamPolicy,
        policy: ResolvedUpstreamPolicy,
    ) -> Result<Client, UpstreamClientError> {
        policy
            .validate()
            .map_err(|_| UpstreamClientError::InvalidPolicy)?;
        let key = UpstreamClientKey::resolve(channel, policy);
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| UpstreamClientError::RegistryUnavailable)?;
        if entries
            .active_keys
            .as_ref()
            .is_some_and(|active_keys| !active_keys.contains(&key))
        {
            drop(entries);
            return build_client(channel, policy);
        }
        if let Some(client) = entries.clients.get(&key).cloned() {
            touch(&mut entries.least_to_most_recent, &key);
            return Ok(client);
        }

        let client = build_client(channel, policy)?;
        if entries.clients.len() == UPSTREAM_CLIENT_REGISTRY_CAPACITY
            && let Some(evicted) = entries.least_to_most_recent.pop_front()
        {
            entries.clients.remove(&evicted);
        }
        entries.least_to_most_recent.push_back(key.clone());
        entries.clients.insert(key, client.clone());
        Ok(client)
    }

    /// Returns a bounded, reusable client for administrator-triggered
    /// diagnostics. Diagnostic drafts are intentionally kept separate from
    /// the forwarding cache so repeated tests reuse connections without
    /// evicting active data-plane clients.
    pub fn diagnostic_client_for(
        &self,
        channel: &CompiledChannelUpstreamPolicy,
        policy: ResolvedUpstreamPolicy,
    ) -> Result<Client, UpstreamClientError> {
        policy
            .validate()
            .map_err(|_| UpstreamClientError::InvalidPolicy)?;
        let key = UpstreamClientKey::resolve(channel, policy);
        let mut entries = self
            .diagnostic_entries
            .lock()
            .map_err(|_| UpstreamClientError::RegistryUnavailable)?;
        if let Some(client) = entries.clients.get(&key).cloned() {
            touch(&mut entries.least_to_most_recent, &key);
            return Ok(client);
        }

        let client = build_client(channel, policy)?;
        if entries.clients.len() == DIAGNOSTIC_CLIENT_REGISTRY_CAPACITY
            && let Some(evicted) = entries.least_to_most_recent.pop_front()
        {
            entries.clients.remove(&evicted);
        }
        entries.least_to_most_recent.push_back(key.clone());
        entries.clients.insert(key, client.clone());
        Ok(client)
    }

    /// Establishes reusable client keys for forwarding channels and enabled
    /// periodic-test channels in `snapshot`, then removes retired cached
    /// clients. A request using an old snapshot receives an ephemeral client
    /// for an inactive key rather than repopulating this cache. Dropping a
    /// cache entry never invalidates a client already cloned by an in-flight
    /// request.
    pub fn reconcile(&self, snapshot: &CompiledRuntimeConfig) -> Result<(), UpstreamClientError> {
        validate_snapshot_upstream_policies(snapshot)
            .map_err(|_| UpstreamClientError::InvalidPolicy)?;
        let upstream_defaults = snapshot.system_settings().upstream_timeouts();
        let active_keys = snapshot
            .probe_channels()
            .map(|channel| {
                UpstreamClientKey::resolve(
                    channel.upstream_policy(),
                    ResolvedUpstreamPolicy::resolve(&upstream_defaults, channel.upstream_policy()),
                )
            })
            .collect::<HashSet<_>>();
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| UpstreamClientError::RegistryUnavailable)?;
        entries.clients.retain(|key, _| active_keys.contains(key));
        let cached = entries.clients.keys().cloned().collect::<HashSet<_>>();
        entries
            .least_to_most_recent
            .retain(|key| cached.contains(key));
        entries.active_keys = Some(active_keys);
        drop(entries);
        self.websockets.reconcile(snapshot);
        Ok(())
    }

    pub(crate) fn configure_websockets(&self, settings: ResponsesWebSocketSettings) {
        self.websockets.configure(settings);
    }

    /// Checks out an idle Responses WebSocket with the exact same client,
    /// channel, network, target, and handshake-header identity.
    #[must_use]
    pub(crate) fn acquire_websocket(
        &self,
        key: &UpstreamWebSocketKey,
    ) -> Option<UpstreamWebSocket> {
        self.websockets.acquire(key)
    }

    /// Returns a clean, completed Responses WebSocket to the bounded idle pool.
    pub(crate) fn release_websocket(
        &self,
        key: UpstreamWebSocketKey,
        connection: UpstreamWebSocket,
    ) {
        self.websockets.release(key, connection);
    }

    pub(crate) fn record_connected_websocket(&self) {
        self.websockets.record_connected();
    }

    pub(crate) fn discard_leased_websocket(&self) {
        self.websockets.discard_leased();
    }

    #[must_use]
    pub(crate) fn websocket_pool_snapshot(&self) -> WebSocketPoolSnapshot {
        self.websockets.snapshot()
    }

    /// Finds the most recently pooled channel for one downstream WebSocket
    /// identity so reconnects can preserve OpenAI's connection-local cache.
    #[must_use]
    pub(crate) fn preferred_websocket_channel(
        &self,
        api_key_id: uuid::Uuid,
        client_identity: WebSocketClientIdentity,
    ) -> Option<uuid::Uuid> {
        self.websockets
            .preferred_channel(api_key_id, client_identity)
    }

    /// Drops every idle Responses WebSocket during process shutdown.
    pub(crate) fn clear_websockets(&self) {
        self.websockets.clear();
    }

    #[cfg(test)]
    fn len(&self) -> Result<usize, UpstreamClientError> {
        self.entries
            .lock()
            .map(|entries| entries.clients.len())
            .map_err(|_| UpstreamClientError::RegistryUnavailable)
    }

    #[cfg(test)]
    fn diagnostic_len(&self) -> Result<usize, UpstreamClientError> {
        self.diagnostic_entries
            .lock()
            .map(|entries| entries.clients.len())
            .map_err(|_| UpstreamClientError::RegistryUnavailable)
    }
}

fn touch(lru: &mut VecDeque<UpstreamClientKey>, key: &UpstreamClientKey) {
    if let Some(index) = lru.iter().position(|entry| entry == key) {
        lru.remove(index);
    }
    lru.push_back(key.clone());
}

fn build_client(
    channel: &CompiledChannelUpstreamPolicy,
    policy: ResolvedUpstreamPolicy,
) -> Result<Client, UpstreamClientError> {
    let builder = Client::builder()
        .connect_timeout(policy.timeouts().connect())
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .use_rustls_tls()
        .tls_built_in_webpki_certs(true);

    let builder = match channel.proxy() {
        None => builder.no_proxy(),
        Some(proxy) => builder.proxy(configured_proxy(proxy)?),
    };
    builder.build().map_err(|_| UpstreamClientError::Build)
}

fn configured_proxy(proxy: &crate::domain::CompiledProxy) -> Result<Proxy, UpstreamClientError> {
    if !matches!(
        proxy.url().scheme(),
        "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
    ) {
        return Err(UpstreamClientError::Build);
    }
    let no_proxy_hosts = proxy.no_proxy_hosts().to_vec();
    let proxy_url = configured_proxy_url(proxy)?;
    let username = proxy.username();
    let password = proxy.password();
    if matches!(proxy.url().scheme(), "socks4" | "socks4a")
        && (username.is_some() || password.is_some())
    {
        return Err(UpstreamClientError::Build);
    }
    if matches!(proxy.url().scheme(), "socks5" | "socks5h")
        && !valid_socks5_credentials(username, password)
    {
        return Err(UpstreamClientError::Build);
    }
    let uses_http_credentials = matches!(proxy.url().scheme(), "http" | "https")
        && (username.is_some() || password.is_some());
    let configured = Proxy::custom(move |target| {
        if proxy_bypasses_target(&no_proxy_hosts, target) {
            None
        } else {
            Some(proxy_url.clone())
        }
    });

    if uses_http_credentials {
        Ok(configured.basic_auth(username.unwrap_or_default(), password.unwrap_or_default()))
    } else {
        Ok(configured)
    }
}

fn proxy_bypasses_target(no_proxy_hosts: &[NoProxyHost], target: &Url) -> bool {
    target.host_str().is_some_and(|host| {
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        no_proxy_hosts.iter().any(|rule| rule.matches_host(host))
    })
}

fn configured_proxy_url(proxy: &crate::domain::CompiledProxy) -> Result<Url, UpstreamClientError> {
    let mut url = proxy.url().clone();
    if matches!(url.scheme(), "socks5" | "socks5h") {
        if let Some(username) = proxy.username() {
            url.set_username(username)
                .map_err(|_| UpstreamClientError::Build)?;
        }
        if let Some(password) = proxy.password() {
            url.set_password(Some(password))
                .map_err(|_| UpstreamClientError::Build)?;
        }
    }
    Ok(url)
}

fn valid_socks5_credentials(username: Option<&str>, password: Option<&str>) -> bool {
    match (username, password) {
        (None, None) => true,
        (Some(username), Some(password)) => {
            (1..=255).contains(&username.len()) && (1..=255).contains(&password.len())
        }
        _ => false,
    }
}

fn credential_free_proxy_url(url: &Url) -> String {
    let mut url = url.clone();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.into()
}

fn credential_fingerprint(username: Option<&str>, password: Option<&str>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-gateway/upstream-client-proxy-credential/v1\\0");
    for value in [username, password] {
        match value {
            Some(value) => {
                hasher.update([1]);
                hasher.update((value.len() as u64).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hasher.finalize().into()
}

fn normalized_no_proxy_host(host: &NoProxyHost) -> Box<str> {
    match host {
        NoProxyHost::ExactDns(name) => format!("exact:{name}").into_boxed_str(),
        NoProxyHost::Ip(address) => format!("ip:{address}").into_boxed_str(),
        NoProxyHost::DnsSuffix(name) => format!("suffix:{name}").into_boxed_str(),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;
    use crate::{
        domain::{
            ApiFormat, ChannelTimeoutPolicy, CompiledChannelUpstreamPolicy, CompiledProxy,
            PassiveHealthSettings, SystemRuntimeSettings, UpstreamTimeoutDefaults,
        },
        persistence::{ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ProxyRecord},
        runtime_config::compile_control_plane_with_system_settings,
        transforms::TransformPlan,
    };
    use uuid::Uuid;

    fn upstream() -> UpstreamTimeoutDefaults {
        UpstreamTimeoutDefaults::new(
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_secs(7),
        )
    }

    fn policy(
        proxy: Option<Arc<CompiledProxy>>,
        timeouts: ChannelTimeoutPolicy,
    ) -> CompiledChannelUpstreamPolicy {
        CompiledChannelUpstreamPolicy::new(
            proxy,
            None,
            TransformPlan::noop(ApiFormat::OpenAiChatCompletions),
            TransformPlan::noop(ApiFormat::OpenAiChatCompletions),
            timeouts,
        )
    }

    fn proxy(
        url: &str,
        username: Option<&str>,
        password: Option<&str>,
        hosts: &[&str],
    ) -> Arc<CompiledProxy> {
        Arc::new(CompiledProxy::new(
            Uuid::new_v4(),
            Arc::from("egress"),
            Url::parse(url).unwrap(),
            username.map(Arc::from),
            password.map(Arc::from),
            hosts
                .iter()
                .map(|host| NoProxyHost::parse(host).unwrap())
                .collect::<Vec<_>>()
                .into(),
        ))
    }

    fn snapshot_with_proxy(proxy_url: &str) -> CompiledRuntimeConfig {
        snapshot_with_proxy_and_timeouts(proxy_url, None, None)
    }

    fn snapshot_with_proxy_and_timeouts(
        proxy_url: &str,
        connect_timeout_ms: Option<i32>,
        response_header_timeout_ms: Option<i32>,
    ) -> CompiledRuntimeConfig {
        let group_id = Uuid::from_u128(1);
        let channel_id = Uuid::from_u128(2);
        let proxy_id = Uuid::from_u128(3);
        compile_control_plane_with_system_settings(
            ControlPlaneRecords {
                api_keys: vec![],
                groups: vec![ChannelGroupRecord {
                    id: group_id,
                    name: "group".into(),
                    api_format: "open_ai_chat_completions".into(),
                    connector_kind: "openai_compatible".into(),
                    priority: 0,
                    selection_strategy: "weighted_random".into(),
                    enabled: true,
                }],
                channels: vec![ChannelRecord {
                    id: channel_id,
                    channel_group_id: group_id,
                    api_format: "open_ai_chat_completions".into(),
                    name: "channel".into(),
                    base_url: "https://upstream.test".into(),
                    enabled: true,
                    supports_websocket: false,
                    auto_disabled: false,
                    auto_disable_allowed: false,
                    weight: 1,
                    billing_multiplier: rust_decimal::Decimal::ONE,
                    proxy_id: Some(proxy_id),
                    config_template_id: None,
                    override_document: serde_json::json!({}),
                    connect_timeout_ms,
                    response_header_timeout_ms,
                    stream_idle_timeout_ms: None,
                    upstream_auth_kind: "none".into(),
                    upstream_auth_header_name: None,
                    upstream_api_key: None,
                    available_models: vec!["upstream".into()],
                    test_model: None,
                }],
                models: vec![],
                model_rules: vec![],
                proxies: vec![ProxyRecord {
                    id: proxy_id,
                    name: "egress".into(),
                    proxy_url: proxy_url.into(),
                    username: None,
                    password: None,
                    no_proxy_hosts: vec![],
                    enabled: true,
                }],
                templates: vec![],
            },
            SystemRuntimeSettings::new(upstream(), PassiveHealthSettings::default()),
        )
        .unwrap()
    }

    #[test]
    fn resolves_channel_timeout_overrides_without_using_non_client_timeouts_in_the_key() {
        let channel = policy(
            None,
            ChannelTimeoutPolicy::new(
                Some(Duration::from_millis(250)),
                Some(Duration::from_millis(500)),
                Some(Duration::from_millis(750)),
            ),
        );
        let resolved = ResolvedUpstreamPolicy::resolve(&upstream(), &channel);
        assert_eq!(resolved.timeouts().connect(), Duration::from_millis(250));
        assert_eq!(
            resolved.timeouts().response_header(),
            Duration::from_millis(500)
        );
        assert_eq!(
            resolved.timeouts().stream_idle(),
            Duration::from_millis(750)
        );

        let different_lifecycle_timeouts = policy(
            None,
            ChannelTimeoutPolicy::new(Some(Duration::from_millis(250)), None, None),
        );
        assert_eq!(
            UpstreamClientKey::resolve(&channel, resolved),
            UpstreamClientKey::resolve(
                &different_lifecycle_timeouts,
                ResolvedUpstreamPolicy::resolve(&upstream(), &different_lifecycle_timeouts),
            )
        );
    }

    #[test]
    fn invalid_effective_timeout_ordering_is_rejected_before_a_client_is_cached() {
        let defaults = upstream();
        let valid = policy(
            None,
            ChannelTimeoutPolicy::new(
                Some(Duration::from_millis(250)),
                Some(Duration::from_millis(500)),
                None,
            ),
        );
        let resolved = ResolvedUpstreamPolicy::try_resolve(&defaults, &valid).unwrap();
        assert_eq!(resolved, ResolvedUpstreamPolicy::resolve(&defaults, &valid));

        let registry = UpstreamClientRegistry::new();
        for invalid in [
            policy(
                None,
                ChannelTimeoutPolicy::new(Some(Duration::from_secs(5)), None, None),
            ),
            policy(
                None,
                ChannelTimeoutPolicy::new(None, Some(Duration::from_secs(2)), None),
            ),
        ] {
            assert_eq!(
                ResolvedUpstreamPolicy::try_resolve(&defaults, &invalid),
                Err(ResolvedUpstreamPolicyError::InvalidTimeoutOrdering)
            );
            assert!(matches!(
                registry.client_for(
                    &invalid,
                    ResolvedUpstreamPolicy::resolve(&defaults, &invalid),
                ),
                Err(UpstreamClientError::InvalidPolicy)
            ));
            assert_eq!(registry.len().unwrap(), 0);
        }
    }

    #[test]
    fn client_key_is_redacted_canonical_and_sensitive_to_network_policy() {
        let first = policy(
            Some(proxy(
                "http://proxy.test:8080",
                Some("user"),
                Some("password-one"),
                &["*.example.test", "::1", "api.example.test"],
            )),
            ChannelTimeoutPolicy::default(),
        );
        let reordered = policy(
            Some(proxy(
                "http://proxy.test:8080/",
                Some("user"),
                Some("password-one"),
                &["api.example.test", "*.example.test", "::1"],
            )),
            ChannelTimeoutPolicy::default(),
        );
        let changed_credentials = policy(
            Some(proxy(
                "http://proxy.test:8080",
                Some("user"),
                Some("password-two"),
                &["*.example.test", "::1", "api.example.test"],
            )),
            ChannelTimeoutPolicy::default(),
        );
        let changed_url = policy(
            Some(proxy(
                "http://other-proxy.test:8080",
                Some("user"),
                Some("password-one"),
                &["*.example.test", "::1", "api.example.test"],
            )),
            ChannelTimeoutPolicy::default(),
        );
        let changed_no_proxy = policy(
            Some(proxy(
                "http://proxy.test:8080",
                Some("user"),
                Some("password-one"),
                &["*.example.test", "::1"],
            )),
            ChannelTimeoutPolicy::default(),
        );
        let slower_connect = policy(
            first.proxy().cloned(),
            ChannelTimeoutPolicy::new(Some(Duration::from_secs(9)), None, None),
        );

        let first_key = UpstreamClientKey::resolve(
            &first,
            ResolvedUpstreamPolicy::resolve(&upstream(), &first),
        );
        assert_eq!(
            first_key,
            UpstreamClientKey::resolve(
                &reordered,
                ResolvedUpstreamPolicy::resolve(&upstream(), &reordered)
            )
        );
        for different in [
            &changed_credentials,
            &changed_url,
            &changed_no_proxy,
            &slower_connect,
        ] {
            assert_ne!(
                first_key,
                UpstreamClientKey::resolve(
                    different,
                    ResolvedUpstreamPolicy::resolve(&upstream(), different)
                )
            );
        }
        let rendered = format!("{first_key:?} {first_key}");
        assert!(!rendered.contains("user"));
        assert!(!rendered.contains("password-one"));
        assert!(!rendered.contains("proxy.test"));
    }

    #[test]
    fn configured_proxy_bypasses_only_explicit_matching_hosts_including_ipv6() {
        let proxy = proxy(
            "http://proxy.test:8080",
            None,
            None,
            &["*.internal.test", "::1"],
        );
        assert!(proxy_bypasses_target(
            proxy.no_proxy_hosts(),
            &Url::parse("https://api.internal.test/path").unwrap()
        ));
        assert!(proxy_bypasses_target(
            proxy.no_proxy_hosts(),
            &Url::parse("https://[::1]/path").unwrap()
        ));
        assert!(!proxy_bypasses_target(
            proxy.no_proxy_hosts(),
            &Url::parse("https://internal.test/path").unwrap()
        ));
        assert!(!proxy_bypasses_target(
            proxy.no_proxy_hosts(),
            &Url::parse("https://public.test/path").unwrap()
        ));
        assert!(!proxy_bypasses_target(
            proxy.no_proxy_hosts(),
            &Url::parse("mailto:unknown-host").unwrap()
        ));
    }

    #[test]
    fn registry_reuses_clients_and_keeps_a_bounded_lru() {
        let registry = UpstreamClientRegistry::new();
        let direct = policy(None, ChannelTimeoutPolicy::default());
        let resolved = ResolvedUpstreamPolicy::resolve(&upstream(), &direct);
        let first = registry.client_for(&direct, resolved).unwrap();
        let second = registry.client_for(&direct, resolved).unwrap();
        assert_eq!(registry.len().unwrap(), 1);
        drop(second);
        drop(first);

        for port in 10_000..(10_000 + UPSTREAM_CLIENT_REGISTRY_CAPACITY as u16 + 1) {
            let channel = policy(
                Some(proxy(&format!("http://proxy.test:{port}"), None, None, &[])),
                ChannelTimeoutPolicy::default(),
            );
            registry
                .client_for(
                    &channel,
                    ResolvedUpstreamPolicy::resolve(&upstream(), &channel),
                )
                .unwrap();
        }
        assert_eq!(registry.len().unwrap(), UPSTREAM_CLIENT_REGISTRY_CAPACITY);
    }

    #[test]
    fn diagnostic_registry_reuses_clients_without_populating_forwarding_entries() {
        let registry = UpstreamClientRegistry::new();
        let diagnostic = policy(
            Some(proxy("http://diagnostic-proxy.test:8080", None, None, &[])),
            ChannelTimeoutPolicy::default(),
        );
        let resolved = ResolvedUpstreamPolicy::resolve(&upstream(), &diagnostic);

        registry
            .diagnostic_client_for(&diagnostic, resolved)
            .unwrap();
        registry
            .diagnostic_client_for(&diagnostic, resolved)
            .unwrap();

        assert_eq!(registry.diagnostic_len().unwrap(), 1);
        assert_eq!(registry.len().unwrap(), 0);
    }

    #[test]
    fn retired_snapshot_policy_uses_an_ephemeral_client_after_reconciliation() {
        let registry = UpstreamClientRegistry::new();
        let initial = snapshot_with_proxy("http://initial-proxy.test:8080");
        let replacement = snapshot_with_proxy("http://replacement-proxy.test:8080");
        let channel_id = Uuid::from_u128(2);
        let initial_channel = initial.channel(channel_id).unwrap();
        let replacement_channel = replacement.channel(channel_id).unwrap();
        let initial_policy = initial_channel.upstream_policy();
        let replacement_policy = replacement_channel.upstream_policy();
        let initial_key = UpstreamClientKey::resolve(
            initial_policy,
            ResolvedUpstreamPolicy::try_resolve(&upstream(), initial_policy).unwrap(),
        );
        let replacement_key = UpstreamClientKey::resolve(
            replacement_policy,
            ResolvedUpstreamPolicy::try_resolve(&upstream(), replacement_policy).unwrap(),
        );
        assert_ne!(initial_key, replacement_key);

        registry.reconcile(&initial).unwrap();
        let old_snapshot_client = registry
            .client_for(
                initial_policy,
                ResolvedUpstreamPolicy::try_resolve(&upstream(), initial_policy).unwrap(),
            )
            .unwrap();
        assert_eq!(registry.len().unwrap(), 1);
        registry.reconcile(&replacement).unwrap();
        assert_eq!(registry.len().unwrap(), 0);
        assert!(
            old_snapshot_client
                .get("https://upstream.test")
                .build()
                .is_ok()
        );

        let retired_snapshot_client = registry
            .client_for(
                initial_policy,
                ResolvedUpstreamPolicy::try_resolve(&upstream(), initial_policy).unwrap(),
            )
            .unwrap();
        assert_eq!(registry.len().unwrap(), 0);
        assert!(
            retired_snapshot_client
                .get("https://upstream.test")
                .build()
                .is_ok()
        );

        registry
            .client_for(
                replacement_policy,
                ResolvedUpstreamPolicy::try_resolve(&upstream(), replacement_policy).unwrap(),
            )
            .unwrap();
        assert_eq!(registry.len().unwrap(), 1);
    }

    #[test]
    fn invalid_snapshot_policy_is_rejected_without_changing_active_registry_keys() {
        let registry = UpstreamClientRegistry::new();
        let valid = snapshot_with_proxy("http://valid-proxy.test:8080");
        let invalid = snapshot_with_proxy_and_timeouts(
            "http://invalid-proxy.test:8080",
            Some(500),
            Some(500),
        );
        let valid_channel = valid.channel(Uuid::from_u128(2)).unwrap();
        let valid_policy = valid_channel.upstream_policy();

        validate_snapshot_upstream_policies(&valid).unwrap();
        assert_eq!(
            validate_snapshot_upstream_policies(&invalid),
            Err(ResolvedUpstreamPolicyError::InvalidTimeoutOrdering)
        );
        registry.reconcile(&valid).unwrap();
        registry
            .client_for(
                valid_policy,
                ResolvedUpstreamPolicy::try_resolve(&upstream(), valid_policy).unwrap(),
            )
            .unwrap();
        assert_eq!(registry.len().unwrap(), 1);

        assert_eq!(
            registry.reconcile(&invalid),
            Err(UpstreamClientError::InvalidPolicy)
        );
        assert_eq!(registry.len().unwrap(), 1);
    }

    #[test]
    fn client_build_errors_are_opaque() {
        let proxy = proxy("ftp://proxy.test:1080", Some("user"), Some("password"), &[]);
        let channel = policy(Some(proxy), ChannelTimeoutPolicy::default());
        let registry = UpstreamClientRegistry::new();
        let error = registry
            .client_for(
                &channel,
                ResolvedUpstreamPolicy::resolve(&upstream(), &channel),
            )
            .unwrap_err();
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("user"));
        assert!(!rendered.contains("password"));
    }
}
