use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::IpAddr,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use reqwest::{Url, header::HeaderName};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use super::{
    ApiFormat, ApiKeyHash, CompiledAdvancedBilling, CompiledMcpServer, ConnectorKind,
    RequestCompression, SystemRuntimeSettings,
};
use crate::transforms::TransformPlan;

/// A normalized `no_proxy_hosts` pattern.
///
/// The accepted grammar is deliberately small and deterministic: an exact
/// ASCII DNS name (`api.example.test`), an IP address (`192.0.2.1` or `::1`),
/// or an ASCII DNS suffix prefixed by `*.` (`*.example.test`). DNS labels are
/// lower-cased, must be 1--63 characters, and consist of ASCII letters,
/// digits, and interior hyphens; names are at most 253 characters. Suffixes
/// match subdomains only, never their apex. Paths, ports, whitespace, bare
/// wildcards, and malformed IP addresses or DNS names are rejected.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum NoProxyHost {
    ExactDns(Arc<str>),
    Ip(IpAddr),
    DnsSuffix(Arc<str>),
}
impl NoProxyHost {
    /// Parses and normalizes a persisted no-proxy host pattern.
    pub fn parse(value: &str) -> Result<Self, NoProxyHostError> {
        if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(NoProxyHostError);
        }
        if let Some(suffix) = value.strip_prefix("*.") {
            return normalize_dns_name(suffix)
                .map(|suffix| Self::DnsSuffix(Arc::from(suffix)))
                .ok_or(NoProxyHostError);
        }
        if value.contains('*') {
            return Err(NoProxyHostError);
        }
        if let Ok(address) = IpAddr::from_str(value) {
            return Ok(Self::Ip(address));
        }
        if looks_like_ipv4_address(value) {
            return Err(NoProxyHostError);
        }
        normalize_dns_name(value)
            .map(|name| Self::ExactDns(Arc::from(name)))
            .ok_or(NoProxyHostError)
    }

    /// Returns whether this pattern bypasses the proxy for `host`.
    #[must_use]
    pub fn matches_host(&self, host: &str) -> bool {
        match self {
            Self::ExactDns(name) => normalize_dns_name(host).is_some_and(|host| host == **name),
            Self::Ip(address) => IpAddr::from_str(host).is_ok_and(|host| host == *address),
            Self::DnsSuffix(suffix) => normalize_dns_name(host).is_some_and(|host| {
                host.len() > suffix.len()
                    && host.ends_with(suffix.as_ref())
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            }),
        }
    }

    /// Returns the normalized DNS name for an exact-name or suffix pattern.
    #[must_use]
    pub fn dns_name(&self) -> Option<&str> {
        match self {
            Self::ExactDns(name) | Self::DnsSuffix(name) => Some(name),
            Self::Ip(_) => None,
        }
    }

    /// Returns the exact IP address when this is an IP pattern.
    #[must_use]
    pub fn ip_addr(&self) -> Option<IpAddr> {
        match self {
            Self::Ip(address) => Some(*address),
            Self::ExactDns(_) | Self::DnsSuffix(_) => None,
        }
    }

    /// Returns whether this pattern matches DNS subdomains rather than one host.
    #[must_use]
    pub fn is_dns_suffix(&self) -> bool {
        matches!(self, Self::DnsSuffix(_))
    }
}

/// Safe, value-free parse error for persisted no-proxy patterns.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("invalid no_proxy host pattern")]
pub struct NoProxyHostError;

fn looks_like_ipv4_address(value: &str) -> bool {
    value.contains('.')
        && value
            .split('.')
            .all(|label| !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_digit()))
}

fn normalize_dns_name(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 253 || !value.is_ascii() || value.ends_with('.') {
        return None;
    }
    let name = value.to_ascii_lowercase();
    name.split('.').all(valid_dns_label).then_some(name)
}

fn valid_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    bytes.len() <= 63
        && !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Immutable, validated outbound proxy configuration. Credential-bearing
/// fields are intentionally redacted from `Debug`.
#[derive(Clone)]
pub struct CompiledProxy {
    id: Uuid,
    name: Arc<str>,
    url: Url,
    username: Option<Arc<str>>,
    password: Option<Arc<str>>,
    no_proxy_hosts: Arc<[NoProxyHost]>,
}
impl CompiledProxy {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }
    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }
    #[must_use]
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }
    #[must_use]
    pub fn no_proxy_hosts(&self) -> &[NoProxyHost] {
        &self.no_proxy_hosts
    }
    pub(crate) fn new(
        id: Uuid,
        name: Arc<str>,
        url: Url,
        username: Option<Arc<str>>,
        password: Option<Arc<str>>,
        no_proxy_hosts: Arc<[NoProxyHost]>,
    ) -> Self {
        Self {
            id,
            name,
            url,
            username,
            password,
            no_proxy_hosts,
        }
    }
}
impl fmt::Debug for CompiledProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledProxy")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url", &self.url)
            .field("username", &self.username.as_ref().map(|_| "REDACTED"))
            .field("password", &self.password.as_ref().map(|_| "REDACTED"))
            .field("no_proxy_hosts", &self.no_proxy_hosts)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct CompiledConfigTemplate {
    id: Uuid,
    name: Arc<str>,
    description: Option<Arc<str>>,
    api_format: Option<ApiFormat>,
    chat_completions_plan: TransformPlan,
    responses_plan: TransformPlan,
    images_plan: TransformPlan,
}
impl CompiledConfigTemplate {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    #[must_use]
    pub fn api_format(&self) -> Option<ApiFormat> {
        self.api_format
    }
    #[must_use]
    pub fn transform_plan(&self, api_format: ApiFormat) -> &TransformPlan {
        match api_format {
            ApiFormat::OpenAiChatCompletions => &self.chat_completions_plan,
            ApiFormat::OpenAiResponses => &self.responses_plan,
            ApiFormat::OpenAiImages => &self.images_plan,
        }
    }
    pub(crate) fn new(
        id: Uuid,
        name: Arc<str>,
        description: Option<Arc<str>>,
        api_format: Option<ApiFormat>,
        chat_completions_plan: TransformPlan,
        responses_plan: TransformPlan,
        images_plan: TransformPlan,
    ) -> Self {
        Self {
            id,
            name,
            description,
            api_format,
            chat_completions_plan,
            responses_plan,
            images_plan,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChannelTimeoutPolicy {
    connect: Option<Duration>,
    response_header: Option<Duration>,
    stream_idle: Option<Duration>,
}

/// Opaque, credential-safe digest of a channel's outbound network policy.
///
/// The raw proxy URL, credentials, no-proxy rules, and connect-timeout override
/// are only used as SHA-256 input and are never exposed through this type.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct OutboundNetworkPolicyFingerprint([u8; 32]);

impl fmt::Debug for OutboundNetworkPolicyFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OutboundNetworkPolicyFingerprint(REDACTED)")
    }
}
impl ChannelTimeoutPolicy {
    #[must_use]
    pub fn connect(&self) -> Option<Duration> {
        self.connect
    }
    #[must_use]
    pub fn response_header(&self) -> Option<Duration> {
        self.response_header
    }
    #[must_use]
    pub fn stream_idle(&self) -> Option<Duration> {
        self.stream_idle
    }
    pub(crate) fn new(
        connect: Option<Duration>,
        response_header: Option<Duration>,
        stream_idle: Option<Duration>,
    ) -> Self {
        Self {
            connect,
            response_header,
            stream_idle,
        }
    }
}

/// The fully compiled per-channel policy used by future outbound client code.
#[derive(Clone, Debug)]
pub struct CompiledChannelUpstreamPolicy {
    proxy: Option<Arc<CompiledProxy>>,
    template: Option<Arc<CompiledConfigTemplate>>,
    channel_override: TransformPlan,
    effective_transforms: TransformPlan,
    timeouts: ChannelTimeoutPolicy,
    outbound_network_policy_fingerprint: OutboundNetworkPolicyFingerprint,
}
impl CompiledChannelUpstreamPolicy {
    #[must_use]
    pub fn proxy(&self) -> Option<&Arc<CompiledProxy>> {
        self.proxy.as_ref()
    }
    #[must_use]
    pub fn template(&self) -> Option<&Arc<CompiledConfigTemplate>> {
        self.template.as_ref()
    }
    #[must_use]
    pub fn channel_override(&self) -> &TransformPlan {
        &self.channel_override
    }
    #[must_use]
    pub fn effective_transforms(&self) -> &TransformPlan {
        &self.effective_transforms
    }
    #[must_use]
    pub fn timeouts(&self) -> &ChannelTimeoutPolicy {
        &self.timeouts
    }
    /// Returns an opaque identity for the outbound network policy.
    #[must_use]
    pub fn outbound_network_policy_fingerprint(&self) -> OutboundNetworkPolicyFingerprint {
        self.outbound_network_policy_fingerprint
    }
    pub(crate) fn new(
        proxy: Option<Arc<CompiledProxy>>,
        template: Option<Arc<CompiledConfigTemplate>>,
        channel_override: TransformPlan,
        effective_transforms: TransformPlan,
        timeouts: ChannelTimeoutPolicy,
    ) -> Self {
        Self::new_with_default_connect_timeout(
            proxy,
            template,
            channel_override,
            effective_transforms,
            timeouts,
            SystemRuntimeSettings::default()
                .upstream_timeouts()
                .connect(),
        )
    }

    pub(crate) fn new_with_default_connect_timeout(
        proxy: Option<Arc<CompiledProxy>>,
        template: Option<Arc<CompiledConfigTemplate>>,
        channel_override: TransformPlan,
        effective_transforms: TransformPlan,
        timeouts: ChannelTimeoutPolicy,
        default_connect_timeout: Duration,
    ) -> Self {
        let outbound_network_policy_fingerprint = outbound_network_policy_fingerprint(
            proxy.as_deref(),
            timeouts.connect().unwrap_or(default_connect_timeout),
        );
        Self {
            proxy,
            template,
            channel_override,
            effective_transforms,
            timeouts,
            outbound_network_policy_fingerprint,
        }
    }

    #[allow(dead_code)] // retained for callers constructing a transparent policy directly
    pub(crate) fn transparent(api_format: ApiFormat) -> Self {
        Self::new(
            None,
            None,
            TransformPlan::noop(api_format),
            TransformPlan::noop(api_format),
            ChannelTimeoutPolicy::default(),
        )
    }
}

fn outbound_network_policy_fingerprint(
    proxy: Option<&CompiledProxy>,
    connect_timeout: Duration,
) -> OutboundNetworkPolicyFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-gateway/outbound-network-policy/v1\0");
    match proxy {
        None => hasher.update([0]),
        Some(proxy) => {
            hasher.update([1]);
            hash_policy_value(&mut hasher, proxy.url().as_str().as_bytes());
            hash_optional_policy_value(&mut hasher, proxy.username());
            hash_optional_policy_value(&mut hasher, proxy.password());
            let mut no_proxy_hosts = proxy
                .no_proxy_hosts()
                .iter()
                .map(canonical_no_proxy_host)
                .collect::<Vec<_>>();
            no_proxy_hosts.sort_unstable();
            hasher.update((no_proxy_hosts.len() as u64).to_be_bytes());
            for host in no_proxy_hosts {
                hash_policy_value(&mut hasher, host.as_bytes());
            }
        }
    }
    hasher.update(connect_timeout.as_secs().to_be_bytes());
    hasher.update(connect_timeout.subsec_nanos().to_be_bytes());
    OutboundNetworkPolicyFingerprint(hasher.finalize().into())
}

fn hash_optional_policy_value(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        None => hasher.update([0]),
        Some(value) => {
            hasher.update([1]);
            hash_policy_value(hasher, value.as_bytes());
        }
    }
}

fn hash_policy_value(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn canonical_no_proxy_host(host: &NoProxyHost) -> String {
    match host {
        NoProxyHost::ExactDns(name) => format!("exact:{name}"),
        NoProxyHost::Ip(address) => format!("ip:{address}"),
        NoProxyHost::DnsSuffix(name) => format!("suffix:{name}"),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
pub enum ApiKeyPermission {
    #[serde(rename = "proxy")]
    Proxy,
    #[serde(rename = "models.read")]
    ModelsRead,
}

/// A deduplicated, immutable routing authorization scope shared by API keys
/// with the same expanded channel access.
#[derive(Clone, Debug)]
pub struct AuthorizationProfile {
    allowed_channel_slots: Arc<[u64]>,
    accessible_route_slots: Arc<[u64]>,
    fingerprint: [u8; 32],
}
impl AuthorizationProfile {
    #[must_use]
    fn permits_route_candidate(&self, slot: usize) -> bool {
        bit_is_set(&self.allowed_channel_slots, slot)
    }

    #[must_use]
    fn permits_route(&self, slot: usize) -> bool {
        bit_is_set(&self.accessible_route_slots, slot)
    }

    #[must_use]
    fn allowed_channel_slots(&self) -> &[u64] {
        &self.allowed_channel_slots
    }

    #[must_use]
    const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    pub(crate) fn new(
        allowed_channel_ids: HashSet<Uuid>,
        allowed_channel_slots: Arc<[u64]>,
        accessible_route_slots: Arc<[u64]>,
    ) -> Self {
        let fingerprint = routing_scope_fingerprint(&HashSet::new(), &allowed_channel_ids);
        Self {
            allowed_channel_slots,
            accessible_route_slots,
            fingerprint,
        }
    }

    #[cfg(test)]
    fn legacy(allowed_group_ids: HashSet<Uuid>, allowed_channel_ids: HashSet<Uuid>) -> Self {
        let fingerprint = routing_scope_fingerprint(&allowed_group_ids, &allowed_channel_ids);
        Self {
            allowed_channel_slots: Arc::from([]),
            accessible_route_slots: Arc::from([]),
            fingerprint,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledApiKey {
    id: Uuid,
    user_id: Uuid,
    websocket_enabled: bool,
    allowed_api_formats: HashSet<ApiFormat>,
    permissions: HashSet<ApiKeyPermission>,
    authorization: Arc<AuthorizationProfile>,
    expires_at: Option<DateTime<Utc>>,
    requests_per_minute: Option<u32>,
    max_concurrent_requests: Option<u32>,
    quota_limit_amount: Option<Decimal>,
    quota_used_amount: Decimal,
}
impl CompiledApiKey {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }
    #[must_use]
    pub const fn websocket_enabled(&self) -> bool {
        self.websocket_enabled
    }
    #[must_use]
    pub fn permits(&self, format: ApiFormat, permission: ApiKeyPermission) -> bool {
        self.allowed_api_formats.contains(&format)
            && self.permissions.contains(&permission)
            && !self.is_expired()
    }
    pub(crate) fn permits_route_candidate(&self, slot: usize) -> bool {
        self.authorization.permits_route_candidate(slot)
    }
    #[must_use]
    pub(crate) fn permits_route(&self, slot: usize) -> bool {
        self.authorization.permits_route(slot)
    }
    #[must_use]
    pub(crate) fn permits_model_capable_route(&self, rule: &CompiledModelRule) -> bool {
        rule.model_capable_candidates_intersect(self.authorization.allowed_channel_slots())
    }
    #[must_use]
    pub(crate) fn routing_scope_fingerprint(&self) -> [u8; 32] {
        self.authorization.fingerprint()
    }
    #[cfg(test)]
    pub(crate) fn shares_authorization_profile(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.authorization, &other.authorization)
    }
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|expires| expires <= Utc::now())
    }
    #[must_use]
    pub fn requests_per_minute(&self) -> Option<u32> {
        self.requests_per_minute
    }
    #[must_use]
    pub fn max_concurrent_requests(&self) -> Option<u32> {
        self.max_concurrent_requests
    }
    #[must_use]
    pub fn quota_limit_amount(&self) -> Option<Decimal> {
        self.quota_limit_amount
    }
    #[must_use]
    pub fn quota_used_amount(&self) -> Decimal {
        self.quota_used_amount
    }
    #[must_use]
    pub fn quota_exhausted(&self) -> bool {
        self.quota_exhausted_at(self.quota_used_amount)
    }
    #[must_use]
    pub fn quota_exhausted_at(&self, quota_used_amount: Decimal) -> bool {
        self.quota_limit_amount
            .is_some_and(|limit| quota_used_amount >= limit)
    }
    #[allow(clippy::too_many_arguments)] // immutable compiled key construction mirrors validated records
    #[cfg(test)]
    pub(crate) fn new(
        id: Uuid,
        user_id: Uuid,
        formats: HashSet<ApiFormat>,
        permissions: HashSet<ApiKeyPermission>,
        groups: HashSet<Uuid>,
        channels: HashSet<Uuid>,
        expires_at: Option<DateTime<Utc>>,
        requests_per_minute: Option<u32>,
        max_concurrent_requests: Option<u32>,
        quota_limit_amount: Option<Decimal>,
        quota_used_amount: Decimal,
    ) -> Self {
        Self::new_with_authorization_profile(
            id,
            user_id,
            false,
            formats,
            permissions,
            Arc::new(AuthorizationProfile::legacy(groups, channels)),
            expires_at,
            requests_per_minute,
            max_concurrent_requests,
            quota_limit_amount,
            quota_used_amount,
        )
    }
    #[allow(clippy::too_many_arguments)] // immutable compiled key construction mirrors validated records
    pub(crate) fn new_with_authorization_profile(
        id: Uuid,
        user_id: Uuid,
        websocket_enabled: bool,
        formats: HashSet<ApiFormat>,
        permissions: HashSet<ApiKeyPermission>,
        authorization: Arc<AuthorizationProfile>,
        expires_at: Option<DateTime<Utc>>,
        requests_per_minute: Option<u32>,
        max_concurrent_requests: Option<u32>,
        quota_limit_amount: Option<Decimal>,
        quota_used_amount: Decimal,
    ) -> Self {
        Self {
            id,
            user_id,
            websocket_enabled,
            allowed_api_formats: formats,
            permissions,
            authorization,
            expires_at,
            requests_per_minute,
            max_concurrent_requests,
            quota_limit_amount,
            quota_used_amount,
        }
    }
    #[cfg(test)]
    pub(crate) fn test_with_policy(
        id: Uuid,
        requests_per_minute: Option<u32>,
        max_concurrent_requests: Option<u32>,
        quota_limit_amount: Option<Decimal>,
        quota_used_amount: Decimal,
    ) -> Self {
        Self::new(
            id,
            Uuid::new_v4(),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            None,
            requests_per_minute,
            max_concurrent_requests,
            quota_limit_amount,
            quota_used_amount,
        )
    }
}

fn routing_scope_fingerprint(groups: &HashSet<Uuid>, channels: &HashSet<Uuid>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-gateway:routing-scope:v1\0");
    let mut groups = groups.iter().copied().collect::<Vec<_>>();
    groups.sort_unstable();
    let mut channels = channels.iter().copied().collect::<Vec<_>>();
    channels.sort_unstable();
    for group in groups {
        hasher.update(group.as_bytes());
    }
    hasher.update([0]);
    for channel in channels {
        hasher.update(channel.as_bytes());
    }
    hasher.finalize().into()
}

fn bit_is_set(words: &[u64], slot: usize) -> bool {
    words
        .get(slot / u64::BITS as usize)
        .is_some_and(|bits| bits & (1_u64 << (slot % u64::BITS as usize)) != 0)
}

#[derive(Clone)]
pub enum UpstreamAuth {
    None,
    Bearer(Arc<str>),
    Header { name: HeaderName, value: Arc<str> },
}
impl fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("UpstreamAuth::None"),
            Self::Bearer(_) => f.write_str("UpstreamAuth::Bearer(REDACTED)"),
            Self::Header { name, .. } => f
                .debug_struct("UpstreamAuth::Header")
                .field("name", name)
                .field("value", &"REDACTED")
                .finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledChannel {
    id: Uuid,
    group_id: Uuid,
    api_format: ApiFormat,
    connector_kind: ConnectorKind,
    request_compression: RequestCompression,
    base_url: Url,
    connectivity_fingerprint: Arc<str>,
    supports_websocket: bool,
    supports_standalone_web_search: bool,
    weight: i32,
    billing_multiplier: Decimal,
    upstream_auth: UpstreamAuth,
    available_models: HashSet<Arc<str>>,
    auto_disable_allowed: bool,
    auto_disabled: bool,
    test_model: Option<Arc<str>>,
    upstream_policy: CompiledChannelUpstreamPolicy,
}
impl CompiledChannel {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn group_id(&self) -> Uuid {
        self.group_id
    }
    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }
    #[must_use]
    pub const fn connector_kind(&self) -> ConnectorKind {
        self.connector_kind
    }
    #[must_use]
    pub const fn request_compression(&self) -> RequestCompression {
        self.request_compression
    }
    #[must_use]
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }
    #[must_use]
    pub(crate) fn connectivity_fingerprint(&self) -> &Arc<str> {
        &self.connectivity_fingerprint
    }
    #[must_use]
    pub const fn supports_websocket(&self) -> bool {
        self.supports_websocket
    }
    #[must_use]
    pub const fn supports_standalone_web_search(&self) -> bool {
        self.supports_standalone_web_search
    }
    #[must_use]
    pub fn weight(&self) -> i32 {
        self.weight
    }
    #[must_use]
    pub fn billing_multiplier(&self) -> Decimal {
        self.billing_multiplier
    }
    #[must_use]
    pub fn upstream_auth(&self) -> &UpstreamAuth {
        &self.upstream_auth
    }
    #[must_use]
    pub fn upstream_policy(&self) -> &CompiledChannelUpstreamPolicy {
        &self.upstream_policy
    }
    #[must_use]
    pub fn supports_model(&self, model: &str) -> bool {
        self.available_models.contains(model)
    }
    #[must_use]
    pub const fn auto_disable_allowed(&self) -> bool {
        self.auto_disable_allowed
    }
    #[must_use]
    pub const fn auto_disabled(&self) -> bool {
        self.auto_disabled
    }
    #[must_use]
    pub fn test_model(&self) -> Option<&str> {
        self.test_model.as_deref()
    }
    #[allow(dead_code)] // compatibility constructor for callers without a policy
    pub(crate) fn new(
        id: Uuid,
        group_id: Uuid,
        api_format: ApiFormat,
        base_url: Url,
        weight: i32,
        upstream_auth: UpstreamAuth,
        available_models: HashSet<Arc<str>>,
    ) -> Self {
        let upstream_policy = CompiledChannelUpstreamPolicy::transparent(api_format);
        Self::new_with_policy(
            id,
            group_id,
            api_format,
            base_url,
            weight,
            upstream_auth,
            available_models,
            upstream_policy,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_policy(
        id: Uuid,
        group_id: Uuid,
        api_format: ApiFormat,
        base_url: Url,
        weight: i32,
        upstream_auth: UpstreamAuth,
        available_models: HashSet<Arc<str>>,
        upstream_policy: CompiledChannelUpstreamPolicy,
    ) -> Self {
        Self::new_with_policy_and_automation(
            id,
            group_id,
            api_format,
            base_url,
            weight,
            upstream_auth,
            available_models,
            false,
            false,
            None,
            upstream_policy,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_policy_and_automation(
        id: Uuid,
        group_id: Uuid,
        api_format: ApiFormat,
        base_url: Url,
        weight: i32,
        upstream_auth: UpstreamAuth,
        available_models: HashSet<Arc<str>>,
        auto_disable_allowed: bool,
        auto_disabled: bool,
        test_model: Option<Arc<str>>,
        upstream_policy: CompiledChannelUpstreamPolicy,
    ) -> Self {
        Self::new_with_policy_automation_and_billing(
            id,
            group_id,
            api_format,
            base_url,
            weight,
            Decimal::ONE,
            upstream_auth,
            available_models,
            false,
            false,
            auto_disable_allowed,
            auto_disabled,
            test_model,
            upstream_policy,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_policy_automation_and_billing(
        id: Uuid,
        group_id: Uuid,
        api_format: ApiFormat,
        base_url: Url,
        weight: i32,
        billing_multiplier: Decimal,
        upstream_auth: UpstreamAuth,
        available_models: HashSet<Arc<str>>,
        supports_websocket: bool,
        supports_standalone_web_search: bool,
        auto_disable_allowed: bool,
        auto_disabled: bool,
        test_model: Option<Arc<str>>,
        upstream_policy: CompiledChannelUpstreamPolicy,
    ) -> Self {
        Self::new_with_connector_policy_automation_and_billing(
            id,
            group_id,
            api_format,
            ConnectorKind::OpenAiCompatible,
            RequestCompression::Default,
            base_url,
            weight,
            billing_multiplier,
            upstream_auth,
            available_models,
            supports_websocket,
            supports_standalone_web_search,
            auto_disable_allowed,
            auto_disabled,
            test_model,
            upstream_policy,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_connector_policy_automation_and_billing(
        id: Uuid,
        group_id: Uuid,
        api_format: ApiFormat,
        connector_kind: ConnectorKind,
        request_compression: RequestCompression,
        base_url: Url,
        weight: i32,
        billing_multiplier: Decimal,
        upstream_auth: UpstreamAuth,
        available_models: HashSet<Arc<str>>,
        supports_websocket: bool,
        supports_standalone_web_search: bool,
        auto_disable_allowed: bool,
        auto_disabled: bool,
        test_model: Option<Arc<str>>,
        upstream_policy: CompiledChannelUpstreamPolicy,
    ) -> Self {
        let connectivity_fingerprint = Arc::from(base_url.as_str());
        Self {
            id,
            group_id,
            api_format,
            connector_kind,
            request_compression,
            base_url,
            connectivity_fingerprint,
            supports_websocket,
            supports_standalone_web_search,
            weight,
            billing_multiplier,
            upstream_auth,
            available_models,
            auto_disable_allowed,
            auto_disabled,
            test_model,
            upstream_policy,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionStrategy {
    WeightedRandom,
    WeightedRoundRobin,
}
impl SelectionStrategy {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "weighted_random" => Some(Self::WeightedRandom),
            "weighted_round_robin" => Some(Self::WeightedRoundRobin),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledChannelGroup {
    id: Uuid,
    api_format: ApiFormat,
    connector_kind: ConnectorKind,
    priority: i32,
    selection_strategy: SelectionStrategy,
}
impl CompiledChannelGroup {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }
    #[must_use]
    pub const fn connector_kind(&self) -> ConnectorKind {
        self.connector_kind
    }
    #[must_use]
    pub fn priority(&self) -> i32 {
        self.priority
    }
    #[must_use]
    pub fn selection_strategy(&self) -> SelectionStrategy {
        self.selection_strategy
    }
    pub(crate) fn new_with_connector(
        id: Uuid,
        api_format: ApiFormat,
        connector_kind: ConnectorKind,
        priority: i32,
        selection_strategy: SelectionStrategy,
    ) -> Self {
        Self {
            id,
            api_format,
            connector_kind,
            priority,
            selection_strategy,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledCandidate {
    channel_slot: usize,
    channel: Arc<CompiledChannel>,
    weight: u32,
}
impl CompiledCandidate {
    #[must_use]
    pub(crate) const fn channel_slot(&self) -> usize {
        self.channel_slot
    }

    #[must_use]
    pub(crate) fn channel(&self) -> &Arc<CompiledChannel> {
        &self.channel
    }

    #[must_use]
    pub(crate) const fn weight(&self) -> u32 {
        self.weight
    }

    pub(crate) fn new(channel_slot: usize, channel: Arc<CompiledChannel>) -> Self {
        Self {
            channel_slot,
            weight: u32::try_from(channel.weight()).expect("compiled positive channel weight"),
            channel,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledRouteTier {
    priority: i32,
    strategy: SelectionStrategy,
    fingerprint: [u8; 32],
    channel_ids: Arc<[Uuid]>,
    candidates: Arc<[CompiledCandidate]>,
}
impl CompiledRouteTier {
    #[must_use]
    pub fn priority(&self) -> i32 {
        self.priority
    }
    #[must_use]
    pub fn strategy(&self) -> SelectionStrategy {
        self.strategy
    }
    #[must_use]
    pub(crate) const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
    #[must_use]
    pub fn channel_ids(&self) -> &[Uuid] {
        &self.channel_ids
    }
    pub(crate) fn candidates(&self) -> &[CompiledCandidate] {
        &self.candidates
    }
    pub(crate) fn new(
        priority: i32,
        strategy: SelectionStrategy,
        candidates: Arc<[CompiledCandidate]>,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"ai-gateway:route-tier:v1\0");
        hasher.update(priority.to_le_bytes());
        hasher.update([match strategy {
            SelectionStrategy::WeightedRandom => 0,
            SelectionStrategy::WeightedRoundRobin => 1,
        }]);
        for candidate in candidates.iter() {
            hasher.update(candidate.channel().id().as_bytes());
            hasher.update(candidate.weight().to_le_bytes());
        }
        let fingerprint = hasher.finalize().into();
        let channel_ids = candidates
            .iter()
            .map(|candidate| candidate.channel().id())
            .collect::<Vec<_>>()
            .into();
        Self {
            priority,
            strategy,
            fingerprint,
            channel_ids,
            candidates,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledModelRule {
    route_slot: usize,
    id: Uuid,
    upstream_model_id: Uuid,
    client_model: Arc<str>,
    api_format: ApiFormat,
    upstream_model: Arc<str>,
    price_snapshot: ModelPriceSnapshot,
    advanced_billing: CompiledAdvancedBilling,
    tiers: Arc<[CompiledRouteTier]>,
    unavailable_candidates: Arc<[CompiledUnavailableRouteCandidate]>,
    target_candidates: Arc<[u64]>,
    model_capable_candidates: Arc<[u64]>,
}

/// Immutable billable-model facts used by scheduled channel tests. Unlike a
/// client route, a test selects its upstream model directly from the channel.
#[derive(Clone, Debug)]
pub struct CompiledScheduledTestModel {
    id: Uuid,
    price_snapshot: ModelPriceSnapshot,
    advanced_billing: CompiledAdvancedBilling,
}

impl CompiledScheduledTestModel {
    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub fn price_snapshot(&self) -> &ModelPriceSnapshot {
        &self.price_snapshot
    }

    #[must_use]
    pub fn advanced_billing(&self) -> &CompiledAdvancedBilling {
        &self.advanced_billing
    }

    pub(crate) fn new(
        id: Uuid,
        price_snapshot: ModelPriceSnapshot,
        advanced_billing: CompiledAdvancedBilling,
    ) -> Self {
        Self {
            id,
            price_snapshot,
            advanced_billing,
        }
    }
}

/// A model-capable route target that is structurally valid but not currently
/// selectable because its group or channel is disabled, or because the channel
/// is automatically disabled.
#[derive(Clone, Debug)]
pub struct CompiledUnavailableRouteCandidate {
    channel_id: Uuid,
    group_id: Uuid,
}

impl CompiledUnavailableRouteCandidate {
    #[must_use]
    pub const fn channel_id(&self) -> Uuid {
        self.channel_id
    }

    #[must_use]
    pub const fn group_id(&self) -> Uuid {
        self.group_id
    }

    pub(crate) fn new(channel_id: Uuid, group_id: Uuid) -> Self {
        Self {
            channel_id,
            group_id,
        }
    }
}

impl CompiledModelRule {
    #[must_use]
    pub(crate) const fn route_slot(&self) -> usize {
        self.route_slot
    }
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn upstream_model_id(&self) -> Uuid {
        self.upstream_model_id
    }
    #[must_use]
    pub fn client_model(&self) -> &str {
        &self.client_model
    }
    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }
    #[must_use]
    pub fn upstream_model(&self) -> &str {
        &self.upstream_model
    }
    #[must_use]
    pub fn price_snapshot(&self) -> &ModelPriceSnapshot {
        &self.price_snapshot
    }
    #[must_use]
    pub fn advanced_billing(&self) -> &CompiledAdvancedBilling {
        &self.advanced_billing
    }
    #[must_use]
    pub fn tiers(&self) -> &[CompiledRouteTier] {
        &self.tiers
    }
    #[must_use]
    pub fn unavailable_candidates(&self) -> &[CompiledUnavailableRouteCandidate] {
        &self.unavailable_candidates
    }
    #[must_use]
    pub(crate) fn authorization_candidates_intersect(&self, allowed_channel_slots: &[u64]) -> bool {
        let candidates = if self.model_capable_candidates.iter().any(|word| *word != 0) {
            &self.model_capable_candidates
        } else {
            &self.target_candidates
        };
        candidates
            .iter()
            .zip(allowed_channel_slots)
            .any(|(candidate, allowed)| candidate & allowed != 0)
    }
    #[must_use]
    pub(crate) fn model_capable_candidates_intersect(&self, allowed_channel_slots: &[u64]) -> bool {
        self.model_capable_candidates
            .iter()
            .zip(allowed_channel_slots)
            .any(|(capable, allowed)| capable & allowed != 0)
    }
    #[must_use]
    pub fn target_candidate_count(&self) -> usize {
        self.target_candidates
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }
    #[must_use]
    pub fn model_capable_candidate_count(&self) -> usize {
        self.model_capable_candidates
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }
    #[must_use]
    pub fn active_candidate_count(&self) -> usize {
        self.tiers.iter().map(|tier| tier.candidates().len()).sum()
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_unavailable_candidates(
        route_slot: usize,
        id: Uuid,
        upstream_model_id: Uuid,
        client_model: Arc<str>,
        api_format: ApiFormat,
        upstream_model: Arc<str>,
        price_snapshot: ModelPriceSnapshot,
        advanced_billing: CompiledAdvancedBilling,
        tiers: Arc<[CompiledRouteTier]>,
        unavailable_candidates: Arc<[CompiledUnavailableRouteCandidate]>,
        target_candidates: Arc<[u64]>,
        model_capable_candidates: Arc<[u64]>,
    ) -> Self {
        Self {
            route_slot,
            id,
            upstream_model_id,
            client_model,
            api_format,
            upstream_model,
            price_snapshot,
            advanced_billing,
            tiers,
            unavailable_candidates,
            target_candidates,
            model_capable_candidates,
        }
    }
}

/// Immutable unit prices selected with a model rule. The request log copies
/// this value so later catalog refreshes cannot reinterpret historical cost.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelPriceSnapshot {
    currency: Arc<str>,
    price_unit_tokens: i64,
    price_effective_at: DateTime<Utc>,
    input_unit_price: Decimal,
    cached_input_unit_price: Decimal,
    cache_write_unit_price: Decimal,
    output_unit_price: Decimal,
}
impl ModelPriceSnapshot {
    #[must_use]
    pub fn currency(&self) -> &str {
        &self.currency
    }
    #[must_use]
    pub fn price_unit_tokens(&self) -> i64 {
        self.price_unit_tokens
    }
    #[must_use]
    pub fn price_effective_at(&self) -> DateTime<Utc> {
        self.price_effective_at
    }
    #[must_use]
    pub fn input_unit_price(&self) -> Decimal {
        self.input_unit_price
    }
    #[must_use]
    pub fn cached_input_unit_price(&self) -> Decimal {
        self.cached_input_unit_price
    }
    #[must_use]
    pub fn cache_write_unit_price(&self) -> Decimal {
        self.cache_write_unit_price
    }
    #[must_use]
    pub fn output_unit_price(&self) -> Decimal {
        self.output_unit_price
    }
    pub(crate) fn new(
        currency: Arc<str>,
        price_unit_tokens: i64,
        price_effective_at: DateTime<Utc>,
        input_unit_price: Decimal,
        cached_input_unit_price: Decimal,
        cache_write_unit_price: Decimal,
        output_unit_price: Decimal,
    ) -> Self {
        Self {
            currency,
            price_unit_tokens,
            price_effective_at,
            input_unit_price,
            cached_input_unit_price,
            cache_write_unit_price,
            output_unit_price,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelRouteKey {
    api_format: ApiFormat,
    client_model: Arc<str>,
}
impl ModelRouteKey {
    #[must_use]
    pub fn new(api_format: ApiFormat, client_model: impl Into<Arc<str>>) -> Self {
        Self {
            api_format,
            client_model: client_model.into(),
        }
    }
}

#[derive(Debug, Default)]
struct CompiledModelRoutes {
    chat_completions: HashMap<Arc<str>, Arc<CompiledModelRule>>,
    responses: HashMap<Arc<str>, Arc<CompiledModelRule>>,
    images: HashMap<Arc<str>, Arc<CompiledModelRule>>,
}
impl CompiledModelRoutes {
    fn from_flat(routes: HashMap<ModelRouteKey, Arc<CompiledModelRule>>) -> Self {
        let mut compiled = Self::default();
        for (key, rule) in routes {
            let target = match key.api_format {
                ApiFormat::OpenAiChatCompletions => &mut compiled.chat_completions,
                ApiFormat::OpenAiResponses => &mut compiled.responses,
                ApiFormat::OpenAiImages => &mut compiled.images,
            };
            target.insert(key.client_model, rule);
        }
        compiled
    }

    fn get(&self, format: ApiFormat, model: &str) -> Option<&Arc<CompiledModelRule>> {
        match format {
            ApiFormat::OpenAiChatCompletions => self.chat_completions.get(model),
            ApiFormat::OpenAiResponses => self.responses.get(model),
            ApiFormat::OpenAiImages => self.images.get(model),
        }
    }

    fn values(&self) -> impl Iterator<Item = &Arc<CompiledModelRule>> {
        self.chat_completions
            .values()
            .chain(self.responses.values())
            .chain(self.images.values())
    }
}

#[derive(Debug)]
pub struct CompiledRuntimeConfig {
    api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
    model_rules: CompiledModelRoutes,
    channels: HashMap<Uuid, Arc<CompiledChannel>>,
    probe_channels: HashMap<Uuid, Arc<CompiledChannel>>,
    scheduled_test_models: HashMap<Arc<str>, Arc<CompiledScheduledTestModel>>,
    groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
    proxies: HashMap<Uuid, Arc<CompiledProxy>>,
    templates: HashMap<Uuid, Arc<CompiledConfigTemplate>>,
    mcp_servers: HashMap<Arc<str>, Arc<CompiledMcpServer>>,
    system_settings: SystemRuntimeSettings,
}
impl CompiledRuntimeConfig {
    #[must_use]
    pub fn new(
        api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
        model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
        channels: HashMap<Uuid, Arc<CompiledChannel>>,
        groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
    ) -> Self {
        Self::with_resources(
            api_keys,
            model_rules,
            channels,
            groups,
            HashMap::new(),
            HashMap::new(),
        )
    }
    #[must_use]
    pub fn with_resources(
        api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
        model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
        channels: HashMap<Uuid, Arc<CompiledChannel>>,
        groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
        proxies: HashMap<Uuid, Arc<CompiledProxy>>,
        templates: HashMap<Uuid, Arc<CompiledConfigTemplate>>,
    ) -> Self {
        Self::with_resources_and_system_settings(
            api_keys,
            model_rules,
            channels,
            groups,
            proxies,
            templates,
            SystemRuntimeSettings::default(),
        )
    }

    #[must_use]
    pub(crate) fn with_resources_and_system_settings(
        api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
        model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
        channels: HashMap<Uuid, Arc<CompiledChannel>>,
        groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
        proxies: HashMap<Uuid, Arc<CompiledProxy>>,
        templates: HashMap<Uuid, Arc<CompiledConfigTemplate>>,
        system_settings: SystemRuntimeSettings,
    ) -> Self {
        let probe_channels = channels.clone();
        Self::with_resources_system_settings_and_probe_channels(
            api_keys,
            model_rules,
            channels,
            probe_channels,
            HashMap::new(),
            groups,
            proxies,
            templates,
            system_settings,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_resources_system_settings_and_probe_channels(
        api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
        model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
        channels: HashMap<Uuid, Arc<CompiledChannel>>,
        probe_channels: HashMap<Uuid, Arc<CompiledChannel>>,
        scheduled_test_models: HashMap<Arc<str>, Arc<CompiledScheduledTestModel>>,
        groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
        proxies: HashMap<Uuid, Arc<CompiledProxy>>,
        templates: HashMap<Uuid, Arc<CompiledConfigTemplate>>,
        system_settings: SystemRuntimeSettings,
    ) -> Self {
        Self {
            api_keys,
            model_rules: CompiledModelRoutes::from_flat(model_rules),
            channels,
            probe_channels,
            scheduled_test_models,
            groups,
            proxies,
            templates,
            mcp_servers: HashMap::new(),
            system_settings,
        }
    }
    #[must_use]
    pub(crate) fn with_mcp_servers(
        mut self,
        mcp_servers: HashMap<Arc<str>, Arc<CompiledMcpServer>>,
    ) -> Self {
        self.mcp_servers = mcp_servers;
        self
    }
    #[must_use]
    pub fn empty() -> Self {
        Self::new(
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        )
    }
    #[must_use]
    pub fn authenticate(&self, secret: &str) -> Option<Arc<CompiledApiKey>> {
        self.api_keys
            .get(&ApiKeyHash::from_secret(secret))
            .filter(|key| !key.is_expired())
            .cloned()
    }
    #[must_use]
    pub fn model_rule(&self, format: ApiFormat, model: &str) -> Option<Arc<CompiledModelRule>> {
        self.model_rules.get(format, model).cloned()
    }
    #[must_use]
    pub fn channel(&self, id: Uuid) -> Option<Arc<CompiledChannel>> {
        self.channels.get(&id).cloned()
    }
    #[must_use]
    pub fn scheduled_test_model(&self, model: &str) -> Option<Arc<CompiledScheduledTestModel>> {
        self.scheduled_test_models.get(model).cloned()
    }
    #[must_use]
    pub fn group(&self, id: Uuid) -> Option<Arc<CompiledChannelGroup>> {
        self.groups.get(&id).cloned()
    }
    #[must_use]
    pub fn proxy(&self, id: Uuid) -> Option<Arc<CompiledProxy>> {
        self.proxies.get(&id).cloned()
    }
    #[must_use]
    pub fn template(&self, id: Uuid) -> Option<Arc<CompiledConfigTemplate>> {
        self.templates.get(&id).cloned()
    }
    #[must_use]
    pub fn mcp_server(&self, slug: &str) -> Option<Arc<CompiledMcpServer>> {
        self.mcp_servers.get(slug).cloned()
    }
    pub fn mcp_servers(&self) -> impl Iterator<Item = &Arc<CompiledMcpServer>> {
        self.mcp_servers.values()
    }
    #[must_use]
    pub fn system_settings(&self) -> &SystemRuntimeSettings {
        &self.system_settings
    }
    pub fn api_keys(&self) -> impl Iterator<Item = &Arc<CompiledApiKey>> {
        self.api_keys.values()
    }
    pub fn model_rules(&self) -> impl Iterator<Item = &Arc<CompiledModelRule>> {
        self.model_rules.values()
    }
    pub fn channels(&self) -> impl Iterator<Item = &Arc<CompiledChannel>> {
        self.channels.values()
    }
    pub fn probe_channels(&self) -> impl Iterator<Item = &Arc<CompiledChannel>> {
        self.probe_channels.values()
    }
    pub fn proxies(&self) -> impl Iterator<Item = &Arc<CompiledProxy>> {
        self.proxies.values()
    }
    pub fn templates(&self) -> impl Iterator<Item = &Arc<CompiledConfigTemplate>> {
        self.templates.values()
    }
    #[must_use]
    pub fn models_for(&self, key: &CompiledApiKey, format: ApiFormat) -> Vec<Arc<str>> {
        if !key.permits(format, ApiKeyPermission::Proxy)
            || !key.permits(format, ApiKeyPermission::ModelsRead)
        {
            return vec![];
        }
        let mut models = self
            .model_rules
            .values()
            .filter(|rule| {
                rule.api_format == format
                    && key.permits_route(rule.route_slot)
                    && key.permits_model_capable_route(rule)
            })
            .map(|rule| Arc::clone(&rule.client_model))
            .collect::<Vec<_>>();
        models.sort_unstable();
        models.dedup();
        models
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::Arc,
    };

    use chrono::{Duration, Utc};
    use reqwest::header::HeaderName;
    use uuid::Uuid;

    use super::{ApiFormat, ApiKeyHash, CompiledApiKey, CompiledRuntimeConfig, UpstreamAuth};

    #[test]
    fn expired_keys_are_rejected_at_authentication_time() {
        let secret = "expired-client-secret";
        let key = Arc::new(CompiledApiKey::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            HashSet::from([ApiFormat::OpenAiChatCompletions]),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            Some(Utc::now() - Duration::seconds(1)),
            None,
            None,
            None,
            rust_decimal::Decimal::ZERO,
        ));
        let snapshot = CompiledRuntimeConfig::new(
            HashMap::from([(ApiKeyHash::from_secret(secret), key)]),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        assert!(snapshot.authenticate(secret).is_none());
    }

    #[test]
    fn custom_upstream_header_debug_redacts_credential() {
        let auth = UpstreamAuth::Header {
            name: HeaderName::from_static("x-api-key"),
            value: Arc::from("upstream-secret"),
        };
        assert!(!format!("{auth:?}").contains("upstream-secret"));
    }
}
