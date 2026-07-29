//! Priority-aware channel selection, process-local session affinity, and
//! snapshot-independent passive health.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::domain::{
    ApiFormat, CompiledApiKey, CompiledChannel, CompiledModelRule, CompiledRouteTier,
    CompiledRuntimeConfig, OutboundNetworkPolicyFingerprint, SelectionStrategy,
};
use serde::Serialize;
use uuid::Uuid;

/// Process-wide policy for passive connection health. These values apply to all
/// channels.
#[derive(Clone, Copy, Debug)]
pub struct PassiveHealthPolicy {
    pub connection_failure_threshold: u32,
    pub cooldown: Duration,
}
impl Default for PassiveHealthPolicy {
    fn default() -> Self {
        Self {
            connection_failure_threshold: 3,
            cooldown: Duration::from_secs(30),
        }
    }
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}
struct SystemClock(Instant);
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// Entropy is injectable so weighted selection is deterministic in tests.
pub trait Entropy: Send + Sync {
    fn next_u64(&self) -> u64;
}
struct ProcessEntropy(std::sync::atomic::AtomicU64);
impl ProcessEntropy {
    fn new() -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        Instant::now().hash(&mut hasher);
        Self(std::sync::atomic::AtomicU64::new(hasher.finish()))
    }
}
impl Entropy for ProcessEntropy {
    fn next_u64(&self) -> u64 {
        use std::sync::atomic::Ordering;
        let mut value = self.0.load(Ordering::Relaxed);
        loop {
            let next = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
            match self
                .0
                .compare_exchange_weak(value, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => {
                    let mut z = next;
                    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                    return z ^ (z >> 31);
                }
                Err(actual) => value = actual,
            }
        }
    }
}

#[derive(Clone)]
pub struct RoutingRuntime {
    inner: Arc<RuntimeInner>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ChannelCapability {
    Any,
    ResponsesWebSocket,
}

impl ChannelCapability {
    fn permits(self, channel: &CompiledChannel) -> bool {
        match self {
            Self::Any => true,
            Self::ResponsesWebSocket => channel.supports_websocket(),
        }
    }
}

struct RuntimeInner {
    policy: Mutex<PassiveHealthPolicy>,
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    channel_states: [RwLock<HashMap<ChannelIdentity, Arc<ChannelState>>>; CHANNEL_STATE_SHARDS],
    active_channels: RwLock<Option<HashSet<ChannelIdentity>>>,
    round_robin: [Mutex<HashMap<RoundRobinKey, HashMap<Uuid, i64>>>; ROUND_ROBIN_SHARDS],
    affinity: Mutex<AffinityState>,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ChannelIdentity {
    id: Uuid,
    connectivity_fingerprint: Arc<str>,
    outbound_network_policy_fingerprint: OutboundNetworkPolicyFingerprint,
}
impl ChannelIdentity {
    fn from_channel(channel: &CompiledChannel) -> Self {
        Self {
            id: channel.id(),
            connectivity_fingerprint: Arc::clone(channel.connectivity_fingerprint()),
            outbound_network_policy_fingerprint: channel
                .upstream_policy()
                .outbound_network_policy_fingerprint(),
        }
    }
}
struct ChannelState {
    in_flight: AtomicU64,
    consecutive_connection_failures: AtomicU32,
    cooldown_state: AtomicU64,
    active: AtomicBool,
}
impl ChannelState {
    fn new(active: bool) -> Self {
        Self {
            in_flight: AtomicU64::new(0),
            consecutive_connection_failures: AtomicU32::new(0),
            cooldown_state: AtomicU64::new(0),
            active: AtomicBool::new(active),
        }
    }

    fn is_usable(&self, now: Duration) -> bool {
        let cooldown_state = self.cooldown_state.load(Ordering::SeqCst);
        cooldown_state == 0
            || (!half_open_claimed(cooldown_state)
                && decode_millis(cooldown_state) <= duration_millis(now))
    }

    fn try_acquire(&self, now: Duration) -> Option<u64> {
        loop {
            let cooldown_state = self.cooldown_state.load(Ordering::SeqCst);
            if cooldown_state == 0 {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                return Some(0);
            }
            if half_open_claimed(cooldown_state)
                || decode_millis(cooldown_state) > duration_millis(now)
            {
                return None;
            }
            let claimed = cooldown_state | HALF_OPEN_CLAIM;
            if self
                .cooldown_state
                .compare_exchange(cooldown_state, claimed, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                return Some(claimed);
            }
        }
    }

    fn release(&self) -> u64 {
        self.in_flight
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1)
    }

    fn connection_failed(&self, threshold: u32, cooldown_until: Duration) {
        let failures = self
            .consecutive_connection_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_add(1))
            })
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        if failures >= threshold {
            self.cooldown_state
                .store(encode_millis(cooldown_until), Ordering::SeqCst);
        }
    }

    fn response_headers_received(&self) {
        self.consecutive_connection_failures
            .store(0, Ordering::SeqCst);
        self.cooldown_state.store(0, Ordering::SeqCst);
    }

    fn probe_failed(&self, half_open_claim: u64, cooldown_until: Duration) {
        if half_open_claim != 0 {
            let _ = self.cooldown_state.compare_exchange(
                half_open_claim,
                encode_millis(cooldown_until),
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }

    fn snapshot(&self, now: Duration) -> ChannelHealthSnapshot {
        let cooldown_state = self.cooldown_state.load(Ordering::SeqCst);
        ChannelHealthSnapshot {
            in_flight: self.in_flight.load(Ordering::SeqCst),
            consecutive_connection_failures: self
                .consecutive_connection_failures
                .load(Ordering::SeqCst),
            cooling_down: cooldown_state != 0
                && decode_millis(cooldown_state) > duration_millis(now),
            half_open_probe: half_open_claimed(cooldown_state),
        }
    }
}

const ROUND_ROBIN_SHARDS: usize = 64;
const CHANNEL_STATE_SHARDS: usize = 64;
const HALF_OPEN_CLAIM: u64 = 1_u64 << 63;
const COOLDOWN_MILLIS_MASK: u64 = HALF_OPEN_CLAIM - 1;

fn duration_millis(value: Duration) -> u64 {
    u64::try_from(value.as_millis())
        .unwrap_or(COOLDOWN_MILLIS_MASK - 1)
        .min(COOLDOWN_MILLIS_MASK - 1)
}

fn encode_millis(value: Duration) -> u64 {
    duration_millis(value).saturating_add(1)
}

fn decode_millis(value: u64) -> u64 {
    (value & COOLDOWN_MILLIS_MASK).saturating_sub(1)
}

fn half_open_claimed(value: u64) -> bool {
    value & HALF_OPEN_CLAIM != 0
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RoundRobinKey {
    rule_id: Uuid,
    priority: i32,
    tier_fingerprint: [u8; 32],
    routing_scope_fingerprint: [u8; 32],
    capability: ChannelCapability,
}

/// A matched and hashed request-side session-affinity rule. Raw extracted
/// values never leave the proxy request stack.
#[derive(Clone, Debug)]
pub struct SessionAffinityMatch {
    rule_name: Arc<str>,
    rule_fingerprint: [u8; 32],
    session_hash: [u8; 32],
    ttl: Duration,
}

impl SessionAffinityMatch {
    #[must_use]
    pub fn new(
        rule_name: Arc<str>,
        rule_fingerprint: [u8; 32],
        session_hash: [u8; 32],
        ttl: Duration,
    ) -> Self {
        Self {
            rule_name,
            rule_fingerprint,
            session_hash,
            ttl,
        }
    }

    #[must_use]
    pub(crate) const fn session_hash(&self) -> [u8; 32] {
        self.session_hash
    }
}

#[derive(Clone, Debug)]
pub struct SessionAffinitySelection {
    rule_name: Arc<str>,
    cache_hit: bool,
}

impl SessionAffinitySelection {
    #[must_use]
    pub fn rule_name(&self) -> &str {
        &self.rule_name
    }

    #[must_use]
    pub const fn cache_hit(&self) -> bool {
        self.cache_hit
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AffinityCacheKey {
    rule_fingerprint: [u8; 32],
    api_key_id: Uuid,
    model_rule_id: Uuid,
    session_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug)]
struct AffinityEntry {
    channel_id: Uuid,
    expires_at: Duration,
    generation: u64,
}

#[derive(Default)]
struct AffinityState {
    enabled: bool,
    max_entries: usize,
    active_rules: HashMap<[u8; 32], Arc<str>>,
    entries: HashMap<AffinityCacheKey, AffinityEntry>,
    recency: VecDeque<(AffinityCacheKey, u64)>,
    next_generation: u64,
}

struct PreparedAffinity {
    key: AffinityCacheKey,
    rule_name: Arc<str>,
    ttl: Duration,
    preferred_channel_id: Option<Uuid>,
}

struct AffinityBinding {
    key: AffinityCacheKey,
    ttl: Duration,
    channel_id: Uuid,
    cache_hit: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelHealthSnapshot {
    pub in_flight: u64,
    pub consecutive_connection_failures: u32,
    pub cooling_down: bool,
    pub half_open_probe: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RoutingPressureSnapshot {
    pub tracked_channels: u64,
    pub in_flight_requests: u64,
    pub cooling_down_channels: u64,
    pub half_open_channels: u64,
    pub session_affinity_entries: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionAffinityRuleCacheSnapshot {
    pub name: String,
    pub entries: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionAffinityCacheSnapshot {
    pub enabled: bool,
    pub max_entries: u64,
    pub total_entries: u64,
    pub rules: Vec<SessionAffinityRuleCacheSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionAffinityCacheClearResult {
    pub cleared_entries: u64,
    pub cache: SessionAffinityCacheSnapshot,
}

impl RoutingRuntime {
    #[must_use]
    pub fn new(policy: PassiveHealthPolicy) -> Self {
        Self::with_seams(
            policy,
            Arc::new(SystemClock(Instant::now())),
            Arc::new(ProcessEntropy::new()),
        )
    }
    #[must_use]
    pub fn with_seams(
        policy: PassiveHealthPolicy,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
    ) -> Self {
        assert!(
            policy.connection_failure_threshold > 0,
            "connection failure threshold must be positive"
        );
        assert!(
            !policy.cooldown.is_zero(),
            "passive health cooldown must be positive"
        );
        Self {
            inner: Arc::new(RuntimeInner {
                policy: Mutex::new(policy),
                clock,
                entropy,
                channel_states: std::array::from_fn(|_| RwLock::new(HashMap::new())),
                active_channels: RwLock::new(None),
                round_robin: std::array::from_fn(|_| Mutex::new(HashMap::new())),
                affinity: Mutex::new(AffinityState::default()),
            }),
        }
    }
    #[must_use]
    pub fn health(&self, channel: &CompiledChannel) -> ChannelHealthSnapshot {
        let now = self.inner.clock.now();
        channel_state(&self.inner, &ChannelIdentity::from_channel(channel)).map_or(
            ChannelHealthSnapshot {
                in_flight: 0,
                consecutive_connection_failures: 0,
                cooling_down: false,
                half_open_probe: false,
            },
            |entry| entry.snapshot(now),
        )
    }

    #[must_use]
    pub fn pressure_snapshot(&self) -> RoutingPressureSnapshot {
        let now = self.inner.clock.now();
        let configured_active_channels = self
            .inner
            .active_channels
            .read()
            .expect("routing active-channel lock poisoned")
            .as_ref()
            .map(HashSet::len);
        let mut tracked_state_channels = 0_usize;
        let mut in_flight_requests = 0_u64;
        let mut cooling_down_channels = 0_u64;
        let mut half_open_channels = 0_u64;
        for shard in &self.inner.channel_states {
            let shard = shard
                .read()
                .expect("routing channel-state shard lock poisoned");
            tracked_state_channels = tracked_state_channels.saturating_add(shard.len());
            for channel in shard.values() {
                let snapshot = channel.snapshot(now);
                in_flight_requests = in_flight_requests.saturating_add(snapshot.in_flight);
                if snapshot.cooling_down {
                    cooling_down_channels = cooling_down_channels.saturating_add(1);
                }
                if snapshot.half_open_probe {
                    half_open_channels = half_open_channels.saturating_add(1);
                }
            }
        }
        let tracked_channels =
            u64::try_from(configured_active_channels.unwrap_or(tracked_state_channels))
                .unwrap_or(u64::MAX);
        let session_affinity_entries = {
            let mut affinity = self
                .inner
                .affinity
                .lock()
                .expect("routing affinity mutex poisoned");
            prune_expired_affinity(&mut affinity, now);
            u64::try_from(affinity.entries.len()).unwrap_or(u64::MAX)
        };
        RoutingPressureSnapshot {
            tracked_channels,
            in_flight_requests,
            cooling_down_channels,
            half_open_channels,
            session_affinity_entries,
        }
    }

    #[must_use]
    pub fn session_affinity_cache_snapshot(&self) -> SessionAffinityCacheSnapshot {
        let now = self.inner.clock.now();
        let mut state = self
            .inner
            .affinity
            .lock()
            .expect("routing affinity mutex poisoned");
        prune_expired_affinity(&mut state, now);
        affinity_cache_snapshot(&state)
    }

    #[must_use]
    pub fn clear_session_affinity_cache(
        &self,
        rule_name: Option<&str>,
    ) -> Option<SessionAffinityCacheClearResult> {
        let now = self.inner.clock.now();
        let mut state = self
            .inner
            .affinity
            .lock()
            .expect("routing affinity mutex poisoned");
        prune_expired_affinity(&mut state, now);

        let before = state.entries.len();
        if let Some(rule_name) = rule_name {
            let fingerprint = state.active_rules.iter().find_map(|(fingerprint, name)| {
                name.as_ref()
                    .eq_ignore_ascii_case(rule_name)
                    .then_some(*fingerprint)
            })?;
            state
                .entries
                .retain(|key, _| key.rule_fingerprint != fingerprint);
        } else {
            state.entries.clear();
        }
        rebuild_affinity_recency(&mut state);
        Some(SessionAffinityCacheClearResult {
            cleared_entries: u64::try_from(before.saturating_sub(state.entries.len()))
                .unwrap_or(u64::MAX),
            cache: affinity_cache_snapshot(&state),
        })
    }

    /// Replaces the process-wide passive-health policy for future connection
    /// failures and half-open probe failures. Existing cooldown deadlines are
    /// intentionally retained; new failure transitions use the new cooldown.
    pub fn update_policy(&self, policy: PassiveHealthPolicy) {
        assert!(
            policy.connection_failure_threshold > 0,
            "connection failure threshold must be positive"
        );
        assert!(
            !policy.cooldown.is_zero(),
            "passive health cooldown must be positive"
        );
        *self
            .inner
            .policy
            .lock()
            .expect("routing policy mutex poisoned") = policy;
    }
    /// Retains runtime health only for channels in the next snapshot, except old
    /// identities which still have an active lease from an earlier snapshot.
    pub fn reconcile(&self, snapshot: &CompiledRuntimeConfig) {
        let active_channels = snapshot
            .channels()
            .map(|channel| ChannelIdentity::from_channel(channel))
            .collect::<HashSet<_>>();
        {
            let mut active = self
                .inner
                .active_channels
                .write()
                .expect("routing active-channel lock poisoned");
            for shard in &self.inner.channel_states {
                shard
                    .write()
                    .expect("routing channel-state shard lock poisoned")
                    .retain(|identity, channel| {
                        let is_active = active_channels.contains(identity);
                        channel.active.store(is_active, Ordering::SeqCst);
                        is_active || channel.in_flight.load(Ordering::SeqCst) > 0
                    });
            }
            for identity in &active_channels {
                self.inner.channel_states[channel_state_shard(identity)]
                    .write()
                    .expect("routing channel-state shard lock poisoned")
                    .entry(identity.clone())
                    .or_insert_with(|| Arc::new(ChannelState::new(true)))
                    .active
                    .store(true, Ordering::SeqCst);
            }
            *active = Some(active_channels);
        }
        // Cursor state is an optimization, not routing health. Clearing it makes
        // successful reload work proportional to active channels and bounds it to
        // selections made in the current generation.
        for shard in &self.inner.round_robin {
            shard
                .lock()
                .expect("routing round-robin mutex poisoned")
                .clear();
        }
        reconcile_affinity(&self.inner, snapshot);
    }
    #[must_use]
    pub fn select(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
    ) -> SelectionResult {
        self.select_with_affinity_excluding(snapshot, key, format, model, None, &[])
    }

    #[must_use]
    pub fn select_with_affinity(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        affinity: Option<SessionAffinityMatch>,
    ) -> SelectionResult {
        self.select_with_affinity_excluding(snapshot, key, format, model, affinity, &[])
    }

    /// Tries to pin a stateful transport to one specific channel while still
    /// enforcing the current model rule, API-key authorization, exclusions,
    /// passive health, highest usable priority tier, and session-affinity
    /// bookkeeping.
    ///
    /// Returns `None` when the preferred channel is no longer eligible. The
    /// caller may then perform ordinary weighted selection.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // mirrors the existing format/model selection surface
    pub fn select_preferred_channel(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        preferred_channel_id: Uuid,
        affinity: Option<SessionAffinityMatch>,
        excluded_channel_slots: &[usize],
    ) -> Option<SelectedRoute> {
        self.select_preferred_channel_with_capability(
            snapshot,
            key,
            format,
            model,
            preferred_channel_id,
            affinity,
            excluded_channel_slots,
            ChannelCapability::Any,
        )
    }

    /// WebSocket-specific preferred selection that excludes channels which do
    /// not explicitly advertise Responses WebSocket support.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn select_preferred_websocket_channel(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        preferred_channel_id: Uuid,
        affinity: Option<SessionAffinityMatch>,
        excluded_channel_slots: &[usize],
    ) -> Option<SelectedRoute> {
        self.select_preferred_channel_with_capability(
            snapshot,
            key,
            format,
            model,
            preferred_channel_id,
            affinity,
            excluded_channel_slots,
            ChannelCapability::ResponsesWebSocket,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn select_preferred_channel_with_capability(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        preferred_channel_id: Uuid,
        affinity: Option<SessionAffinityMatch>,
        excluded_channel_slots: &[usize],
        capability: ChannelCapability,
    ) -> Option<SelectedRoute> {
        let rule = snapshot.model_rule(format, model)?;
        if !key.permits_route(rule.route_slot()) {
            return None;
        }
        let affinity = prepare_affinity(&self.inner, key, &rule, affinity);
        let now = self.inner.clock.now();
        let eligible = |candidate: &crate::domain::CompiledCandidate| {
            let channel_slot = candidate.channel_slot();
            let channel = candidate.channel();
            capability.permits(channel)
                && key.permits_route_candidate(channel_slot)
                && !excluded_channel_slots.contains(&channel_slot)
                && usable(&self.inner, &ChannelIdentity::from_channel(channel), now)
        };
        let mut preferred = None;
        for tier in rule.tiers() {
            if !tier.candidates().iter().any(&eligible) {
                continue;
            }
            preferred = tier.candidates().iter().find(|candidate| {
                candidate.channel().id() == preferred_channel_id && eligible(candidate)
            });
            break;
        }
        let candidate = preferred?;
        let channel_slot = candidate.channel_slot();
        let channel = Arc::clone(candidate.channel());
        let identity = ChannelIdentity::from_channel(&channel);
        let (channel_state, half_open_claim) = try_acquire_channel(&self.inner, &identity, now)?;
        let cache_hit = affinity
            .as_ref()
            .and_then(|affinity| affinity.preferred_channel_id)
            == Some(channel.id());
        let affinity_binding = affinity.as_ref().map(|affinity| AffinityBinding {
            key: affinity.key,
            ttl: affinity.ttl,
            channel_id: channel.id(),
            cache_hit,
        });
        let affinity_selection = affinity.as_ref().map(|affinity| SessionAffinitySelection {
            rule_name: Arc::clone(&affinity.rule_name),
            cache_hit,
        });
        if let Some(affinity) = &affinity
            && let Some(stale_channel_id) = affinity
                .preferred_channel_id
                .filter(|channel_id| *channel_id != channel.id())
        {
            affinity_remove_if_channel(&self.inner, affinity.key, stale_channel_id);
        }
        Some(SelectedRoute {
            rule,
            channel,
            channel_slot,
            session_affinity: affinity_selection,
            lease: ChannelLease {
                inner: Arc::clone(&self.inner),
                identity,
                state: channel_state,
                half_open_claim,
                affinity: affinity_binding,
                released: false,
            },
        })
    }

    /// Selects a route while excluding channels already attempted by the same
    /// client request. Exclusions are applied after authorization and before
    /// priority/weight selection, so failover exhausts each priority tier
    /// without retrying a channel twice.
    #[must_use]
    pub fn select_with_affinity_excluding(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        affinity: Option<SessionAffinityMatch>,
        excluded_channel_slots: &[usize],
    ) -> SelectionResult {
        self.select_with_affinity_excluding_capability(
            snapshot,
            key,
            format,
            model,
            affinity,
            excluded_channel_slots,
            ChannelCapability::Any,
        )
    }

    /// WebSocket-specific weighted selection that excludes channels which do
    /// not explicitly advertise Responses WebSocket support.
    #[must_use]
    pub fn select_websocket_with_affinity_excluding(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        affinity: Option<SessionAffinityMatch>,
        excluded_channel_slots: &[usize],
    ) -> SelectionResult {
        self.select_with_affinity_excluding_capability(
            snapshot,
            key,
            format,
            model,
            affinity,
            excluded_channel_slots,
            ChannelCapability::ResponsesWebSocket,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn select_with_affinity_excluding_capability(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
        affinity: Option<SessionAffinityMatch>,
        excluded_channel_slots: &[usize],
        capability: ChannelCapability,
    ) -> SelectionResult {
        let Some(rule) = snapshot.model_rule(format, model) else {
            return SelectionResult::UnknownOrInaccessibleModel;
        };
        if !key.permits_route(rule.route_slot()) {
            return SelectionResult::UnknownOrInaccessibleModel;
        }
        let affinity = prepare_affinity(&self.inner, key, &rule, affinity);
        let now = self.inner.clock.now();
        for tier in rule.tiers() {
            let mut allow_affinity = true;
            loop {
                let (channel, cache_hit) = {
                    let affinity_channel = allow_affinity
                        .then(|| {
                            affinity
                                .as_ref()
                                .and_then(|affinity| affinity.preferred_channel_id)
                                .and_then(|preferred| {
                                    tier.candidates().iter().find_map(|candidate| {
                                        let slot = candidate.channel_slot();
                                        let channel = candidate.channel();
                                        (channel.id() == preferred
                                            && capability.permits(channel)
                                            && key.permits_route_candidate(slot)
                                            && !excluded_channel_slots.contains(&slot)
                                            && usable(
                                                &self.inner,
                                                &ChannelIdentity::from_channel(channel),
                                                now,
                                            ))
                                        .then(|| (slot, Arc::clone(channel)))
                                    })
                                })
                        })
                        .flatten();
                    let cache_hit = affinity_channel.is_some();
                    let channel = affinity_channel.or_else(|| match tier.strategy() {
                        SelectionStrategy::WeightedRandom => weighted_ticket(
                            tier,
                            key,
                            excluded_channel_slots,
                            &self.inner,
                            now,
                            &*self.inner.entropy,
                            capability,
                        ),
                        SelectionStrategy::WeightedRoundRobin => {
                            let round_robin_key = RoundRobinKey {
                                rule_id: rule.id(),
                                priority: tier.priority(),
                                tier_fingerprint: tier.fingerprint(),
                                routing_scope_fingerprint: key.routing_scope_fingerprint(),
                                capability,
                            };
                            let candidates_are_active = tier
                                .candidates()
                                .iter()
                                .filter(|candidate| {
                                    capability.permits(candidate.channel())
                                        && key.permits_route_candidate(candidate.channel_slot())
                                        && !excluded_channel_slots
                                            .contains(&candidate.channel_slot())
                                })
                                .all(|candidate| {
                                    channel_is_active(
                                        &self.inner,
                                        &ChannelIdentity::from_channel(candidate.channel()),
                                    )
                                });
                            if candidates_are_active {
                                let shard = round_robin_shard(&round_robin_key);
                                let mut shard = self.inner.round_robin[shard]
                                    .lock()
                                    .expect("routing round-robin mutex poisoned");
                                smooth_round_robin(
                                    shard.entry(round_robin_key).or_default(),
                                    tier,
                                    key,
                                    excluded_channel_slots,
                                    &self.inner,
                                    now,
                                    capability,
                                )
                            } else {
                                // A request may still hold an old snapshot after reload.
                                // It can route and hold a lease, but cannot grow state for
                                // a retired connectivity identity.
                                smooth_round_robin(
                                    &mut HashMap::new(),
                                    tier,
                                    key,
                                    excluded_channel_slots,
                                    &self.inner,
                                    now,
                                    capability,
                                )
                            }
                        }
                    });
                    (channel, cache_hit)
                };
                let Some((channel_slot, channel)) = channel else {
                    break;
                };
                let identity = ChannelIdentity::from_channel(&channel);
                let Some((channel_state, half_open_claim)) =
                    try_acquire_channel(&self.inner, &identity, now)
                else {
                    // Another request won the half-open probe between candidate
                    // inspection and lease acquisition. Re-evaluate this tier.
                    allow_affinity = false;
                    continue;
                };
                let affinity_binding = affinity.as_ref().map(|affinity| AffinityBinding {
                    key: affinity.key,
                    ttl: affinity.ttl,
                    channel_id: channel.id(),
                    cache_hit,
                });
                let affinity_selection =
                    affinity.as_ref().map(|affinity| SessionAffinitySelection {
                        rule_name: Arc::clone(&affinity.rule_name),
                        cache_hit,
                    });
                let stale_affinity = affinity
                    .as_ref()
                    .and_then(|affinity| affinity.preferred_channel_id)
                    .filter(|_| !cache_hit);
                let selected = SelectedRoute {
                    rule,
                    channel,
                    channel_slot,
                    session_affinity: affinity_selection,
                    lease: ChannelLease {
                        inner: Arc::clone(&self.inner),
                        identity,
                        state: channel_state,
                        half_open_claim,
                        affinity: affinity_binding,
                        released: false,
                    },
                };
                if let (Some(affinity), Some(channel_id)) = (&affinity, stale_affinity) {
                    affinity_remove_if_channel(&self.inner, affinity.key, channel_id);
                }
                return SelectionResult::Selected(selected);
            }
        }
        if let Some(affinity) = &affinity
            && let Some(channel_id) = affinity.preferred_channel_id
        {
            affinity_remove_if_channel(&self.inner, affinity.key, channel_id);
        }
        SelectionResult::NoHealthyChannel { rule }
    }
}

fn prepare_affinity(
    inner: &RuntimeInner,
    key: &CompiledApiKey,
    rule: &CompiledModelRule,
    affinity: Option<SessionAffinityMatch>,
) -> Option<PreparedAffinity> {
    affinity.map(|affinity| {
        let cache_key = AffinityCacheKey {
            rule_fingerprint: affinity.rule_fingerprint,
            api_key_id: key.id(),
            model_rule_id: rule.id(),
            session_hash: affinity.session_hash,
        };
        PreparedAffinity {
            preferred_channel_id: affinity_lookup(inner, cache_key),
            key: cache_key,
            rule_name: affinity.rule_name,
            ttl: affinity.ttl,
        }
    })
}

fn reconcile_affinity(inner: &RuntimeInner, snapshot: &CompiledRuntimeConfig) {
    let settings = snapshot.system_settings().session_affinity();
    let active_rules = settings
        .rules()
        .iter()
        .map(|rule| (rule.fingerprint(), Arc::from(rule.name())))
        .collect::<HashMap<_, _>>();
    let now = inner.clock.now();
    let mut state = inner
        .affinity
        .lock()
        .expect("session affinity mutex poisoned");
    state.enabled = settings.enabled();
    state.max_entries = settings.max_entries();
    if !state.enabled {
        state.active_rules = active_rules;
        state.entries.clear();
        state.recency.clear();
        return;
    }
    if state.active_rules != active_rules {
        state.active_rules = active_rules;
        let active_rules = state.active_rules.clone();
        state.entries.retain(|key, entry| {
            active_rules.contains_key(&key.rule_fingerprint) && entry.expires_at > now
        });
        rebuild_affinity_recency(&mut state);
    }
    trim_affinity_state(&mut state);
}

fn affinity_lookup(inner: &RuntimeInner, key: AffinityCacheKey) -> Option<Uuid> {
    let now = inner.clock.now();
    let mut state = inner
        .affinity
        .lock()
        .expect("session affinity mutex poisoned");
    if !state.enabled || !state.active_rules.contains_key(&key.rule_fingerprint) {
        return None;
    }
    let entry = state.entries.get(&key).copied()?;
    if entry.expires_at <= now {
        state.entries.remove(&key);
        return None;
    }
    Some(entry.channel_id)
}

fn affinity_store(inner: &RuntimeInner, binding: &AffinityBinding) {
    let now = inner.clock.now();
    let mut state = inner
        .affinity
        .lock()
        .expect("session affinity mutex poisoned");
    if !state.enabled
        || state.max_entries == 0
        || !state
            .active_rules
            .contains_key(&binding.key.rule_fingerprint)
    {
        return;
    }
    state.next_generation = state.next_generation.wrapping_add(1).max(1);
    let generation = state.next_generation;
    state.entries.insert(
        binding.key,
        AffinityEntry {
            channel_id: binding.channel_id,
            expires_at: now + binding.ttl,
            generation,
        },
    );
    state.recency.push_back((binding.key, generation));
    trim_affinity_state(&mut state);
    compact_affinity_recency(&mut state);
}

fn affinity_remove_if_channel(
    inner: &RuntimeInner,
    key: AffinityCacheKey,
    expected_channel_id: Uuid,
) {
    let mut state = inner
        .affinity
        .lock()
        .expect("session affinity mutex poisoned");
    if state
        .entries
        .get(&key)
        .is_some_and(|entry| entry.channel_id == expected_channel_id)
    {
        state.entries.remove(&key);
    }
}

fn trim_affinity_state(state: &mut AffinityState) {
    while state.entries.len() > state.max_entries {
        let Some((key, generation)) = state.recency.pop_front() else {
            if let Some(key) = state.entries.keys().next().copied() {
                state.entries.remove(&key);
                continue;
            }
            break;
        };
        if state
            .entries
            .get(&key)
            .is_some_and(|entry| entry.generation == generation)
        {
            state.entries.remove(&key);
        }
    }
}

fn prune_expired_affinity(state: &mut AffinityState, now: Duration) {
    let before = state.entries.len();
    state.entries.retain(|key, entry| {
        state.active_rules.contains_key(&key.rule_fingerprint) && entry.expires_at > now
    });
    if state.entries.len() != before {
        rebuild_affinity_recency(state);
    }
}

fn affinity_cache_snapshot(state: &AffinityState) -> SessionAffinityCacheSnapshot {
    let mut counts = HashMap::<[u8; 32], u64>::new();
    for key in state.entries.keys() {
        let count = counts.entry(key.rule_fingerprint).or_default();
        *count = count.saturating_add(1);
    }
    let mut rules = state
        .active_rules
        .iter()
        .map(|(fingerprint, name)| SessionAffinityRuleCacheSnapshot {
            name: name.to_string(),
            entries: counts.get(fingerprint).copied().unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    rules.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    SessionAffinityCacheSnapshot {
        enabled: state.enabled,
        max_entries: u64::try_from(state.max_entries).unwrap_or(u64::MAX),
        total_entries: u64::try_from(state.entries.len()).unwrap_or(u64::MAX),
        rules,
    }
}

fn compact_affinity_recency(state: &mut AffinityState) {
    let threshold = state.entries.len().saturating_mul(4).saturating_add(1_024);
    if state.recency.len() <= threshold {
        return;
    }
    rebuild_affinity_recency(state);
}

fn rebuild_affinity_recency(state: &mut AffinityState) {
    let mut entries = state
        .entries
        .iter()
        .map(|(key, entry)| (*key, entry.generation))
        .collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(_, generation)| *generation);
    state.recency = entries.into();
}

fn try_acquire_channel(
    inner: &RuntimeInner,
    identity: &ChannelIdentity,
    now: Duration,
) -> Option<(Arc<ChannelState>, u64)> {
    let channel =
        channel_state(inner, identity).unwrap_or_else(|| channel_state_or_insert(inner, identity));
    let half_open_claim = channel.try_acquire(now)?;
    Some((channel, half_open_claim))
}

fn channel_state(inner: &RuntimeInner, identity: &ChannelIdentity) -> Option<Arc<ChannelState>> {
    inner.channel_states[channel_state_shard(identity)]
        .read()
        .expect("routing channel-state shard lock poisoned")
        .get(identity)
        .cloned()
}

fn channel_state_or_insert(inner: &RuntimeInner, identity: &ChannelIdentity) -> Arc<ChannelState> {
    if let Some(channel) = channel_state(inner, identity) {
        return channel;
    }
    let active_channels = inner
        .active_channels
        .read()
        .expect("routing active-channel lock poisoned");
    let active = active_channels
        .as_ref()
        .is_none_or(|channels| channels.contains(identity));
    Arc::clone(
        inner.channel_states[channel_state_shard(identity)]
            .write()
            .expect("routing channel-state shard lock poisoned")
            .entry(identity.clone())
            .or_insert_with(|| Arc::new(ChannelState::new(active))),
    )
}

fn channel_state_shard(identity: &ChannelIdentity) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    identity.hash(&mut hasher);
    (hasher.finish() % CHANNEL_STATE_SHARDS as u64) as usize
}

fn round_robin_shard(key: &RoundRobinKey) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() % ROUND_ROBIN_SHARDS as u64) as usize
}

fn channel_is_active(inner: &RuntimeInner, identity: &ChannelIdentity) -> bool {
    let shard = inner.channel_states[channel_state_shard(identity)]
        .read()
        .expect("routing channel-state shard lock poisoned");
    if let Some(channel) = shard.get(identity) {
        return channel.active.load(Ordering::SeqCst);
    }
    drop(shard);
    inner
        .active_channels
        .read()
        .expect("routing active-channel lock poisoned")
        .as_ref()
        .is_none_or(|channels| channels.contains(identity))
}

fn usable(inner: &RuntimeInner, identity: &ChannelIdentity, now: Duration) -> bool {
    inner.channel_states[channel_state_shard(identity)]
        .read()
        .expect("routing channel-state shard lock poisoned")
        .get(identity)
        .is_none_or(|channel| channel.is_usable(now))
}

fn weighted_ticket(
    tier: &CompiledRouteTier,
    key: &CompiledApiKey,
    excluded_channel_slots: &[usize],
    inner: &RuntimeInner,
    now: Duration,
    entropy: &dyn Entropy,
    capability: ChannelCapability,
) -> Option<(usize, Arc<CompiledChannel>)> {
    let eligible = |slot: usize, channel: &CompiledChannel| {
        capability.permits(channel)
            && key.permits_route_candidate(slot)
            && !excluded_channel_slots.contains(&slot)
            && usable(inner, &ChannelIdentity::from_channel(channel), now)
    };
    loop {
        let total = tier
            .candidates()
            .iter()
            .filter(|candidate| eligible(candidate.channel_slot(), candidate.channel()))
            .map(|candidate| u64::from(candidate.weight()))
            .sum::<u64>();
        if total == 0 {
            return None;
        }
        let zone = u64::MAX - (u64::MAX % total);
        let ticket = loop {
            let value = entropy.next_u64();
            if value < zone {
                break value % total;
            }
        };
        let mut remaining = ticket;
        let mut observed_total = 0_u64;
        let mut selected = None;
        for candidate in tier.candidates() {
            let slot = candidate.channel_slot();
            let channel = candidate.channel();
            if !eligible(slot, channel) {
                continue;
            }
            let weight = u64::from(candidate.weight());
            observed_total += weight;
            if selected.is_none() && remaining < weight {
                selected = Some((slot, Arc::clone(channel)));
            } else if selected.is_none() {
                remaining -= weight;
            }
        }
        if observed_total == total {
            return selected;
        }
        // Channel health may change between the two allocation-free scans.
        // Retry with a fresh total so the ticket always corresponds to the
        // exact candidate set observed by the selection pass.
    }
}

fn smooth_round_robin(
    current: &mut HashMap<Uuid, i64>,
    tier: &CompiledRouteTier,
    key: &CompiledApiKey,
    excluded_channel_slots: &[usize],
    inner: &RuntimeInner,
    now: Duration,
    capability: ChannelCapability,
) -> Option<(usize, Arc<CompiledChannel>)> {
    let mut total = 0_i64;
    let mut winner = None::<(usize, Arc<CompiledChannel>, i64)>;
    for candidate in tier.candidates() {
        let slot = candidate.channel_slot();
        let channel = candidate.channel();
        if !capability.permits(channel)
            || !key.permits_route_candidate(slot)
            || excluded_channel_slots.contains(&slot)
            || !usable(inner, &ChannelIdentity::from_channel(channel), now)
        {
            continue;
        }
        total += i64::from(candidate.weight());
        let value = current.entry(channel.id()).or_insert(0);
        *value += i64::from(candidate.weight());
        if winner
            .as_ref()
            .is_none_or(|(_, _, winner_value)| *value > *winner_value)
        {
            winner = Some((slot, Arc::clone(channel), *value));
        }
    }
    let (slot, winner, _) = winner?;
    *current.get_mut(&winner.id()).expect("winner exists") -= total;
    Some((slot, winner))
}

#[allow(clippy::large_enum_variant)] // keep successful selection free of request-level boxing
pub enum SelectionResult {
    UnknownOrInaccessibleModel,
    NoHealthyChannel { rule: Arc<CompiledModelRule> },
    Selected(SelectedRoute),
}
impl SelectionResult {
    #[must_use]
    pub fn is_none(&self) -> bool {
        matches!(
            self,
            Self::UnknownOrInaccessibleModel | Self::NoHealthyChannel { .. }
        )
    }
}

pub struct SelectedRoute {
    pub rule: Arc<CompiledModelRule>,
    pub channel: Arc<CompiledChannel>,
    pub channel_slot: usize,
    pub session_affinity: Option<SessionAffinitySelection>,
    pub lease: ChannelLease,
}

/// Releases in-flight accounting when the selected response completes or is cancelled.
/// It is deliberately non-cloneable so every selection has exactly one lease owner.
pub struct ChannelLease {
    inner: Arc<RuntimeInner>,
    identity: ChannelIdentity,
    state: Arc<ChannelState>,
    half_open_claim: u64,
    affinity: Option<AffinityBinding>,
    released: bool,
}
impl ChannelLease {
    pub fn request_succeeded(&mut self) {
        if let Some(affinity) = self.affinity.as_ref() {
            affinity_store(&self.inner, affinity);
        }
    }

    pub fn request_failed(&mut self) {
        if let Some(affinity) = self.affinity.as_ref()
            && affinity.cache_hit
        {
            affinity_remove_if_channel(&self.inner, affinity.key, affinity.channel_id);
        }
    }

    pub fn response_headers_received(&mut self) {
        self.state.response_headers_received();
    }
    pub fn connection_failed(&mut self) {
        let now = self.inner.clock.now();
        let policy = *self
            .inner
            .policy
            .lock()
            .expect("routing policy mutex poisoned");
        self.state
            .connection_failed(policy.connection_failure_threshold, now + policy.cooldown);
    }
    /// A half-open request reached neither response headers nor a known-success
    /// state. Reopen the cooldown without treating ordinary header timeouts as
    /// connection failures.
    pub fn probe_failed(&mut self) {
        if self.half_open_claim == 0 {
            return;
        }
        let now = self.inner.clock.now();
        let policy = *self
            .inner
            .policy
            .lock()
            .expect("routing policy mutex poisoned");
        self.state
            .probe_failed(self.half_open_claim, now + policy.cooldown);
    }
}
impl Drop for ChannelLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let remaining = self.state.release();
        // Cancellation is neutral. Explicit failed-probe transitions reopen cooldown.
        if self.half_open_claim != 0 {
            let _ = self.state.cooldown_state.compare_exchange(
                self.half_open_claim,
                self.half_open_claim & COOLDOWN_MILLIS_MASK,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        if remaining == 0 && !self.state.active.load(Ordering::SeqCst) {
            let mut shard = self.inner.channel_states[channel_state_shard(&self.identity)]
                .write()
                .expect("routing channel-state shard lock poisoned");
            let remove = shard.get(&self.identity).is_some_and(|current| {
                Arc::ptr_eq(current, &self.state)
                    && current.in_flight.load(Ordering::SeqCst) == 0
                    && !current.active.load(Ordering::SeqCst)
            });
            if remove {
                shard.remove(&self.identity);
            }
        }
        self.released = true;
    }
}

/// Compatibility entry point for direct callers. The data plane injects a
/// process-wide runtime through `RoutingRuntime::select` instead.
#[must_use]
pub fn select(
    snapshot: &CompiledRuntimeConfig,
    key: &CompiledApiKey,
    format: ApiFormat,
    model: &str,
) -> SelectionResult {
    RoutingRuntime::new(PassiveHealthPolicy::default()).select(snapshot, key, format, model)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use crate::{
        domain::{
            ApiFormat, AutomaticDisableSettings, CompiledRuntimeConfig, PassiveHealthSettings,
            ScheduledTestingSettings, SessionAffinityRule, SessionAffinitySettings,
            SystemRuntimeSettings, UpstreamTimeoutDefaults,
        },
        persistence::{
            ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
            ProxyRecord,
        },
        runtime_config::{compile_control_plane, compile_control_plane_with_system_settings},
    };
    use regex::Regex;
    use uuid::Uuid;

    use super::{
        Clock, Entropy, PassiveHealthPolicy, RoutingRuntime, SelectionResult, SessionAffinityMatch,
    };

    struct TestClock(AtomicU64);
    impl TestClock {
        fn advance(&self, duration: Duration) {
            self.0
                .fetch_add(duration.as_millis() as u64, Ordering::Relaxed);
        }
    }
    impl Clock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Relaxed))
        }
    }
    struct Tickets(Mutex<VecDeque<u64>>);
    impl Entropy for Tickets {
        fn next_u64(&self) -> u64 {
            self.0.lock().unwrap().pop_front().unwrap_or(0)
        }
    }

    fn snapshot(groups: &[(i32, &str)], weights: &[i32]) -> (CompiledRuntimeConfig, String) {
        snapshot_with_base(groups, weights, None)
    }

    fn snapshot_with_base(
        groups: &[(i32, &str)],
        weights: &[i32],
        base_url: Option<&str>,
    ) -> (CompiledRuntimeConfig, String) {
        snapshot_with_base_and_settings(groups, weights, base_url, SystemRuntimeSettings::default())
    }

    fn snapshot_with_base_and_settings(
        groups: &[(i32, &str)],
        weights: &[i32],
        base_url: Option<&str>,
        system_settings: SystemRuntimeSettings,
    ) -> (CompiledRuntimeConfig, String) {
        assert_eq!(groups.len(), weights.len());
        let group_ids = (0..groups.len())
            .map(|index| Uuid::from_u128(index as u128 + 1))
            .collect::<Vec<_>>();
        let channel_ids = (0..groups.len())
            .map(|index| Uuid::from_u128(index as u128 + 100))
            .collect::<Vec<_>>();
        let secret = "routing-test-key".to_owned();
        let records = ControlPlaneRecords {
            api_keys: vec![ApiKeyRecord {
                id: Uuid::from_u128(1_000),
                user_id: Uuid::from_u128(1_001),
                user_status: "active".into(),
                user_websocket_enabled: false,
                secret_value: secret.clone(),
                status: "active".into(),
                expires_at: None,
                allowed_api_formats: vec!["open_ai_chat_completions".into()],
                permissions: vec!["proxy".into()],
                allowed_group_ids: group_ids.clone(),
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
                quota_used_amount: Default::default(),
            }],
            groups: group_ids
                .iter()
                .zip(groups)
                .map(|(id, (priority, group_strategy))| ChannelGroupRecord {
                    id: *id,
                    name: id.to_string(),
                    api_format: "open_ai_chat_completions".into(),
                    connector_kind: "openai_compatible".into(),
                    priority: *priority,
                    selection_strategy: (*group_strategy).into(),
                    enabled: true,
                })
                .collect(),
            channels: channel_ids
                .iter()
                .zip(group_ids.iter())
                .zip(weights)
                .map(|((id, group_id), weight)| ChannelRecord {
                    id: *id,
                    channel_group_id: *group_id,
                    api_format: "open_ai_chat_completions".into(),
                    name: id.to_string(),
                    base_url: base_url
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("https://{id}.test")),
                    enabled: true,
                    supports_websocket: false,
                    auto_disabled: false,
                    auto_disable_allowed: false,
                    weight: *weight,
                    billing_multiplier: rust_decimal::Decimal::ONE,
                    proxy_id: None,
                    config_template_id: None,
                    override_document: serde_json::json!({}),
                    connect_timeout_ms: None,
                    response_header_timeout_ms: None,
                    stream_idle_timeout_ms: None,
                    upstream_auth_kind: "none".into(),
                    upstream_auth_header_name: None,
                    upstream_api_key: None,
                    available_models: vec!["upstream".into()],
                    test_model: None,
                })
                .collect(),
            models: vec![],
            model_rules: vec![ModelRuleRecord {
                id: Uuid::from_u128(1_002),
                client_model: "model".into(),
                api_format: "open_ai_chat_completions".into(),
                upstream_model_id: Uuid::from_u128(1_003),
                upstream_model_enabled: true,
                upstream_model_currency: "USD".into(),
                price_unit_tokens: 1_000_000,
                price_effective_at: chrono::Utc::now(),
                input_unit_price: Default::default(),
                cached_input_unit_price: Default::default(),
                cache_write_unit_price: Default::default(),
                output_unit_price: Default::default(),
                advanced_billing: serde_json::json!({
                    "long_context_tiers": [],
                    "request_multipliers": [],
                }),
                upstream_model: "upstream".into(),
                channel_group_ids: group_ids,
                channel_ids: vec![],
                enabled: true,
            }],
            proxies: vec![],
            templates: vec![],
        };
        (
            compile_control_plane_with_system_settings(records, system_settings).unwrap(),
            secret,
        )
    }

    fn affinity_system_settings(fingerprint: [u8; 32], ttl: Duration) -> SystemRuntimeSettings {
        SystemRuntimeSettings::new_with_all(
            UpstreamTimeoutDefaults::default(),
            crate::domain::RequestRetrySettings::default(),
            PassiveHealthSettings::default(),
            AutomaticDisableSettings::default(),
            ScheduledTestingSettings::default(),
            SessionAffinitySettings::new(
                true,
                100,
                ttl,
                vec![SessionAffinityRule::new(
                    Arc::from("test-affinity"),
                    fingerprint,
                    vec![ApiFormat::OpenAiChatCompletions].into(),
                    vec![Regex::new("^model$").unwrap()].into(),
                    Vec::new().into(),
                    None,
                    ttl,
                )]
                .into(),
            ),
        )
    }

    fn select(
        runtime: &RoutingRuntime,
        snapshot: &CompiledRuntimeConfig,
        secret: &str,
    ) -> super::SelectedRoute {
        let key = snapshot.authenticate(secret).unwrap();
        match runtime.select(snapshot, &key, ApiFormat::OpenAiChatCompletions, "model") {
            SelectionResult::Selected(route) => route,
            _ => panic!("fixture must select a route"),
        }
    }

    fn round_robin_len(runtime: &RoutingRuntime) -> usize {
        runtime
            .inner
            .round_robin
            .iter()
            .map(|shard| shard.lock().unwrap().len())
            .sum()
    }

    fn snapshot_with_outbound_policy(
        proxy_url: &str,
        username: &str,
        password: &str,
        no_proxy_hosts: &[&str],
        connect_timeout_ms: Option<i32>,
    ) -> (CompiledRuntimeConfig, String) {
        let group_id = Uuid::from_u128(1);
        let channel_id = Uuid::from_u128(100);
        let proxy_id = Uuid::from_u128(200);
        let secret = "routing-network-policy-key".to_owned();
        let records = ControlPlaneRecords {
            api_keys: vec![ApiKeyRecord {
                id: Uuid::from_u128(300),
                user_id: Uuid::from_u128(301),
                user_status: "active".into(),
                user_websocket_enabled: false,
                secret_value: secret.clone(),
                status: "active".into(),
                expires_at: None,
                allowed_api_formats: vec!["open_ai_chat_completions".into()],
                permissions: vec!["proxy".into()],
                allowed_group_ids: vec![group_id],
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
                quota_used_amount: Default::default(),
            }],
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
                response_header_timeout_ms: None,
                stream_idle_timeout_ms: None,
                upstream_auth_kind: "none".into(),
                upstream_auth_header_name: None,
                upstream_api_key: None,
                available_models: vec!["upstream".into()],
                test_model: None,
            }],
            models: vec![],
            model_rules: vec![ModelRuleRecord {
                id: Uuid::from_u128(400),
                client_model: "model".into(),
                api_format: "open_ai_chat_completions".into(),
                upstream_model_id: Uuid::from_u128(401),
                upstream_model_enabled: true,
                upstream_model_currency: "USD".into(),
                price_unit_tokens: 1_000_000,
                price_effective_at: chrono::Utc::now(),
                input_unit_price: Default::default(),
                cached_input_unit_price: Default::default(),
                cache_write_unit_price: Default::default(),
                output_unit_price: Default::default(),
                advanced_billing: serde_json::json!({
                    "long_context_tiers": [],
                    "request_multipliers": [],
                }),
                upstream_model: "upstream".into(),
                channel_group_ids: vec![],
                channel_ids: vec![channel_id],
                enabled: true,
            }],
            proxies: vec![ProxyRecord {
                id: proxy_id,
                name: "egress".into(),
                proxy_url: proxy_url.into(),
                username: Some(username.into()),
                password: Some(password.into()),
                no_proxy_hosts: no_proxy_hosts.iter().map(|host| (*host).into()).collect(),
                enabled: true,
            }],
            templates: vec![],
        };
        (compile_control_plane(records).unwrap(), secret)
    }

    #[test]
    fn priority_then_authorized_tier_selects_lowest_usable_channel() {
        let (snapshot, secret) =
            snapshot(&[(10, "weighted_random"), (0, "weighted_random")], &[1, 1]);
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            clock,
            Arc::new(Tickets(Mutex::new(VecDeque::from([0])))),
        );
        let route = select(&runtime, &snapshot, &secret);
        assert_eq!(route.rule.tiers()[0].priority(), 0);
        assert_eq!(route.channel.id(), route.rule.tiers()[0].channel_ids()[0]);
    }

    #[test]
    fn stateful_transport_can_pin_a_preferred_channel_within_the_active_priority_tier() {
        let (flat_snapshot, secret) =
            snapshot(&[(0, "weighted_random"), (0, "weighted_random")], &[1, 1]);
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::from([0])))),
        );
        runtime.reconcile(&flat_snapshot);
        let key = flat_snapshot.authenticate(&secret).unwrap();
        let rule = flat_snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "model")
            .unwrap();
        let preferred = rule.tiers()[0].channel_ids()[1];

        let selected = runtime
            .select_preferred_channel(
                &flat_snapshot,
                &key,
                ApiFormat::OpenAiChatCompletions,
                "model",
                preferred,
                None,
                &[],
            )
            .expect("preferred channel should remain eligible");
        assert_eq!(selected.channel.id(), preferred);
        let preferred_slot = selected.channel_slot;
        drop(selected);

        assert!(
            runtime
                .select_preferred_channel(
                    &flat_snapshot,
                    &key,
                    ApiFormat::OpenAiChatCompletions,
                    "model",
                    preferred,
                    None,
                    &[preferred_slot],
                )
                .is_none()
        );

        let (tiered_snapshot, tiered_secret) =
            snapshot(&[(0, "weighted_random"), (10, "weighted_random")], &[1, 1]);
        runtime.reconcile(&tiered_snapshot);
        let tiered_key = tiered_snapshot.authenticate(&tiered_secret).unwrap();
        let tiered_rule = tiered_snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "model")
            .unwrap();
        assert!(
            runtime
                .select_preferred_channel(
                    &tiered_snapshot,
                    &tiered_key,
                    ApiFormat::OpenAiChatCompletions,
                    "model",
                    tiered_rule.tiers()[1].channel_ids()[0],
                    None,
                    &[],
                )
                .is_none(),
            "stateful reuse must not bypass a healthier higher-priority tier"
        );
    }

    #[test]
    fn weighted_random_uses_exact_ticket_boundaries() {
        let (snapshot, secret) =
            snapshot(&[(0, "weighted_random"), (0, "weighted_random")], &[2, 3]);
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::from([0, 1, 2, 3, 4])))),
        );
        let expected = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "model")
            .unwrap()
            .tiers()[0]
            .channel_ids()
            .to_vec();
        let selected = (0..5)
            .map(|_| select(&runtime, &snapshot, &secret).channel.id())
            .collect::<Vec<_>>();
        assert_eq!(
            selected,
            vec![
                expected[0],
                expected[0],
                expected[1],
                expected[1],
                expected[1]
            ]
        );
    }

    #[test]
    fn successful_session_affinity_reuses_the_selected_channel() {
        let fingerprint = [7; 32];
        let ttl = Duration::from_secs(60);
        let (snapshot, secret) = snapshot_with_base_and_settings(
            &[(0, "weighted_random"), (0, "weighted_random")],
            &[1, 1],
            None,
            affinity_system_settings(fingerprint, ttl),
        );
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::from([0, 1])))),
        );
        runtime.reconcile(&snapshot);
        let key = snapshot.authenticate(&secret).unwrap();
        let affinity =
            || SessionAffinityMatch::new(Arc::from("test-affinity"), fingerprint, [9; 32], ttl);

        let mut first = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("first affinity request must select"),
        };
        assert!(!first.session_affinity.as_ref().unwrap().cache_hit());
        let first_channel = first.channel.id();
        first.lease.request_succeeded();
        drop(first);

        let second = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("second affinity request must select"),
        };
        assert!(second.session_affinity.as_ref().unwrap().cache_hit());
        assert_eq!(second.channel.id(), first_channel);
    }

    #[test]
    fn session_affinity_cache_reports_only_valid_entries_and_supports_manual_clear() {
        let fingerprint = [6; 32];
        let ttl = Duration::from_secs(60);
        let (snapshot, secret) = snapshot_with_base_and_settings(
            &[(0, "weighted_random"), (0, "weighted_random")],
            &[1, 1],
            None,
            affinity_system_settings(fingerprint, ttl),
        );
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            clock.clone(),
            Arc::new(Tickets(Mutex::new(VecDeque::from([0, 1])))),
        );
        runtime.reconcile(&snapshot);
        let key = snapshot.authenticate(&secret).unwrap();
        let affinity =
            || SessionAffinityMatch::new(Arc::from("test-affinity"), fingerprint, [8; 32], ttl);

        let mut selected = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("affinity request must select"),
        };
        selected.lease.request_succeeded();
        drop(selected);

        let report = runtime.session_affinity_cache_snapshot();
        assert_eq!(report.total_entries, 1);
        assert_eq!(report.rules[0].name, "test-affinity");
        assert_eq!(report.rules[0].entries, 1);
        assert!(
            runtime
                .clear_session_affinity_cache(Some("missing-rule"))
                .is_none()
        );

        let cleared = runtime
            .clear_session_affinity_cache(Some("TEST-AFFINITY"))
            .unwrap();
        assert_eq!(cleared.cleared_entries, 1);
        assert_eq!(cleared.cache.total_entries, 0);

        let mut selected = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("affinity request must select after clear"),
        };
        selected.lease.request_succeeded();
        drop(selected);
        clock.advance(Duration::from_secs(61));

        let expired = runtime.session_affinity_cache_snapshot();
        assert_eq!(expired.total_entries, 0);
        assert_eq!(expired.rules[0].entries, 0);
    }

    #[test]
    fn failed_affinity_hit_is_removed_before_the_next_selection() {
        let fingerprint = [8; 32];
        let ttl = Duration::from_secs(60);
        let (snapshot, secret) = snapshot_with_base_and_settings(
            &[(0, "weighted_random"), (0, "weighted_random")],
            &[1, 1],
            None,
            affinity_system_settings(fingerprint, ttl),
        );
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::from([0, 1])))),
        );
        runtime.reconcile(&snapshot);
        let key = snapshot.authenticate(&secret).unwrap();
        let affinity =
            || SessionAffinityMatch::new(Arc::from("test-affinity"), fingerprint, [10; 32], ttl);

        let mut first = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("first affinity request must select"),
        };
        let first_channel = first.channel.id();
        first.lease.request_succeeded();
        drop(first);

        let mut hit = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("affinity hit must select"),
        };
        assert!(hit.session_affinity.as_ref().unwrap().cache_hit());
        hit.lease.request_failed();
        drop(hit);

        let next = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("post-failure request must select"),
        };
        assert!(!next.session_affinity.as_ref().unwrap().cache_hit());
        assert_ne!(next.channel.id(), first_channel);
    }

    #[test]
    fn expired_session_affinity_returns_to_weighted_selection() {
        let fingerprint = [11; 32];
        let ttl = Duration::from_secs(60);
        let (snapshot, secret) = snapshot_with_base_and_settings(
            &[(0, "weighted_random"), (0, "weighted_random")],
            &[1, 1],
            None,
            affinity_system_settings(fingerprint, ttl),
        );
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            clock.clone(),
            Arc::new(Tickets(Mutex::new(VecDeque::from([0, 1])))),
        );
        runtime.reconcile(&snapshot);
        let key = snapshot.authenticate(&secret).unwrap();
        let affinity =
            || SessionAffinityMatch::new(Arc::from("test-affinity"), fingerprint, [12; 32], ttl);

        let mut first = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("first affinity request must select"),
        };
        let first_channel = first.channel.id();
        first.lease.request_succeeded();
        drop(first);

        clock.advance(Duration::from_secs(61));
        let after_expiry = match runtime.select_with_affinity(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            "model",
            Some(affinity()),
        ) {
            SelectionResult::Selected(route) => route,
            _ => panic!("expired affinity request must select"),
        };
        assert!(!after_expiry.session_affinity.as_ref().unwrap().cache_hit());
        assert_ne!(after_expiry.channel.id(), first_channel);
    }

    #[test]
    fn smooth_weighted_round_robin_has_a_deterministic_sequence() {
        let (snapshot, secret) = snapshot(
            &[(0, "weighted_round_robin"), (0, "weighted_round_robin")],
            &[2, 1],
        );
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );
        let ids = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "model")
            .unwrap()
            .tiers()[0]
            .channel_ids()
            .to_vec();
        let selected = (0..6)
            .map(|_| select(&runtime, &snapshot, &secret).channel.id())
            .collect::<Vec<_>>();
        assert_eq!(
            selected,
            vec![ids[0], ids[1], ids[0], ids[0], ids[1], ids[0]]
        );
    }

    #[test]
    fn breaker_cooldown_admits_one_probe_and_headers_recover() {
        let (snapshot, secret) = snapshot(&[(0, "weighted_random")], &[1]);
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy {
                connection_failure_threshold: 2,
                cooldown: Duration::from_secs(10),
            },
            clock.clone(),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );
        let channel = snapshot
            .channel(
                snapshot
                    .model_rule(ApiFormat::OpenAiChatCompletions, "model")
                    .unwrap()
                    .tiers()[0]
                    .channel_ids()[0],
            )
            .unwrap();
        let mut first = select(&runtime, &snapshot, &secret);
        first.lease.connection_failed();
        drop(first);
        let mut second = select(&runtime, &snapshot, &secret);
        second.lease.connection_failed();
        drop(second);
        assert!(matches!(
            runtime.select(
                &snapshot,
                &snapshot.authenticate(&secret).unwrap(),
                ApiFormat::OpenAiChatCompletions,
                "model"
            ),
            SelectionResult::NoHealthyChannel { .. }
        ));
        clock.advance(Duration::from_secs(10));
        let mut probe = select(&runtime, &snapshot, &secret);
        assert!(matches!(
            runtime.select(
                &snapshot,
                &snapshot.authenticate(&secret).unwrap(),
                ApiFormat::OpenAiChatCompletions,
                "model"
            ),
            SelectionResult::NoHealthyChannel { .. }
        ));
        probe.lease.response_headers_received();
        drop(probe);
        let health = runtime.health(&channel);
        assert_eq!(health.consecutive_connection_failures, 0);
        assert!(!health.cooling_down);
    }

    #[test]
    fn half_open_header_timeout_reopens_cooldown_before_release() {
        let (snapshot, secret) = snapshot(&[(0, "weighted_random")], &[1]);
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy {
                connection_failure_threshold: 1,
                cooldown: Duration::from_secs(10),
            },
            clock.clone(),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );
        let mut failed = select(&runtime, &snapshot, &secret);
        failed.lease.connection_failed();
        drop(failed);

        clock.advance(Duration::from_secs(10));
        let mut probe = select(&runtime, &snapshot, &secret);
        probe.lease.probe_failed();
        drop(probe);

        assert!(matches!(
            runtime.select(
                &snapshot,
                &snapshot.authenticate(&secret).unwrap(),
                ApiFormat::OpenAiChatCompletions,
                "model"
            ),
            SelectionResult::NoHealthyChannel { .. }
        ));
        clock.advance(Duration::from_secs(10));
        assert!(matches!(
            runtime.select(
                &snapshot,
                &snapshot.authenticate(&secret).unwrap(),
                ApiFormat::OpenAiChatCompletions,
                "model"
            ),
            SelectionResult::Selected(_)
        ));
    }

    #[test]
    fn concurrent_cooldown_expiry_admits_exactly_one_half_open_probe() {
        let (snapshot, secret) = snapshot(&[(0, "weighted_random")], &[1]);
        let snapshot = Arc::new(snapshot);
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy {
                connection_failure_threshold: 1,
                cooldown: Duration::from_secs(10),
            },
            clock.clone(),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );
        let mut failed = select(&runtime, &snapshot, &secret);
        failed.lease.connection_failed();
        drop(failed);
        clock.advance(Duration::from_secs(10));

        const WORKERS: usize = 16;
        let start = Arc::new(Barrier::new(WORKERS));
        let finish = Arc::new(Barrier::new(WORKERS));
        let selected = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..WORKERS {
                let runtime = runtime.clone();
                let snapshot = Arc::clone(&snapshot);
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                let selected = Arc::clone(&selected);
                let secret = secret.clone();
                scope.spawn(move || {
                    let key = snapshot.authenticate(&secret).unwrap();
                    start.wait();
                    let route =
                        runtime.select(&snapshot, &key, ApiFormat::OpenAiChatCompletions, "model");
                    if matches!(&route, SelectionResult::Selected(_)) {
                        selected.fetch_add(1, Ordering::SeqCst);
                    }
                    finish.wait();
                    drop(route);
                });
            }
        });
        assert_eq!(selected.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reconciliation_preserves_connectivity_and_discards_reconfigured_idle_state() {
        let (initial, secret) = snapshot_with_base(
            &[(0, "weighted_random")],
            &[1],
            Some("https://initial.test"),
        );
        let (unchanged, _) = snapshot_with_base(
            &[(0, "weighted_random")],
            &[1],
            Some("https://initial.test"),
        );
        let (reconfigured, _) = snapshot_with_base(
            &[(0, "weighted_random")],
            &[1],
            Some("https://replacement.test"),
        );
        let runtime = RoutingRuntime::new(PassiveHealthPolicy::default());
        let old_channel = initial
            .channel(
                initial
                    .model_rule(ApiFormat::OpenAiChatCompletions, "model")
                    .unwrap()
                    .tiers()[0]
                    .channel_ids()[0],
            )
            .unwrap();

        let mut first = select(&runtime, &initial, &secret);
        first.lease.connection_failed();
        drop(first);
        runtime.reconcile(&unchanged);
        assert_eq!(
            runtime.health(&old_channel).consecutive_connection_failures,
            1
        );

        let mut in_flight = select(&runtime, &unchanged, &secret);
        in_flight.lease.connection_failed();
        runtime.reconcile(&reconfigured);
        let replacement = reconfigured
            .channel(
                reconfigured
                    .model_rule(ApiFormat::OpenAiChatCompletions, "model")
                    .unwrap()
                    .tiers()[0]
                    .channel_ids()[0],
            )
            .unwrap();
        assert_eq!(
            runtime.health(&replacement).consecutive_connection_failures,
            0
        );
        assert_eq!(runtime.health(&old_channel).in_flight, 1);

        drop(in_flight);
        runtime.reconcile(&reconfigured);
        assert_eq!(
            runtime.health(&old_channel).consecutive_connection_failures,
            0
        );
    }

    #[test]
    fn reconciliation_isolates_in_flight_failures_for_each_outbound_network_policy() {
        let (initial, secret) = snapshot_with_outbound_policy(
            "http://proxy.test:8080",
            "user",
            "password-one",
            &["*.internal.test"],
            None,
        );
        let initial_channel = initial.channel(Uuid::from_u128(100)).unwrap();
        let initial_fingerprint = initial_channel
            .upstream_policy()
            .outbound_network_policy_fingerprint();

        for (proxy_url, username, password, no_proxy_hosts, connect_timeout_ms) in [
            (
                "http://proxy.test:8080",
                "user",
                "password-two",
                vec!["*.internal.test"],
                None,
            ),
            (
                "http://replacement-proxy.test:8080",
                "user",
                "password-one",
                vec!["*.internal.test"],
                None,
            ),
            (
                "http://proxy.test:8080",
                "user",
                "password-one",
                vec!["*.replacement.internal.test"],
                None,
            ),
            (
                "http://proxy.test:8080",
                "user",
                "password-one",
                vec!["*.internal.test"],
                Some(500),
            ),
        ] {
            let (replacement, _) = snapshot_with_outbound_policy(
                proxy_url,
                username,
                password,
                &no_proxy_hosts,
                connect_timeout_ms,
            );
            let replacement_channel = replacement.channel(Uuid::from_u128(100)).unwrap();
            assert_ne!(
                initial_fingerprint,
                replacement_channel
                    .upstream_policy()
                    .outbound_network_policy_fingerprint()
            );

            let runtime = RoutingRuntime::with_seams(
                PassiveHealthPolicy {
                    connection_failure_threshold: 1,
                    cooldown: Duration::from_secs(10),
                },
                Arc::new(TestClock(AtomicU64::new(0))),
                Arc::new(Tickets(Mutex::new(VecDeque::new()))),
            );
            let mut old = select(&runtime, &initial, &secret);
            runtime.reconcile(&replacement);
            old.lease.connection_failed();

            assert!(runtime.health(&initial_channel).cooling_down);
            assert_eq!(runtime.health(&replacement_channel).in_flight, 0);
            assert_eq!(
                runtime
                    .health(&replacement_channel)
                    .consecutive_connection_failures,
                0
            );
            assert!(matches!(
                runtime.select(
                    &replacement,
                    &replacement.authenticate(&secret).unwrap(),
                    ApiFormat::OpenAiChatCompletions,
                    "model"
                ),
                SelectionResult::Selected(_)
            ));
            drop(old);
        }
    }

    #[test]
    fn round_robin_state_is_not_keyed_by_temporary_health_subsets() {
        let (snapshot, secret) = snapshot(
            &[(0, "weighted_round_robin"), (0, "weighted_round_robin")],
            &[1, 1],
        );
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy {
                connection_failure_threshold: 1,
                cooldown: Duration::from_secs(10),
            },
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );
        let mut first = select(&runtime, &snapshot, &secret);
        first.lease.connection_failed();
        drop(first);
        let second = select(&runtime, &snapshot, &secret);
        drop(second);

        assert_eq!(round_robin_len(&runtime), 1);
    }

    #[test]
    fn reconcile_clears_cursors_and_stale_snapshots_cannot_restore_them() {
        let (old, secret) = snapshot_with_base(
            &[(0, "weighted_round_robin"), (0, "weighted_round_robin")],
            &[1, 1],
            Some("https://old.test"),
        );
        let (next, _) = snapshot_with_base(
            &[(0, "weighted_round_robin"), (0, "weighted_round_robin")],
            &[1, 1],
            Some("https://new.test"),
        );
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );

        drop(select(&runtime, &old, &secret));
        assert_eq!(round_robin_len(&runtime), 1);

        runtime.reconcile(&next);
        assert_eq!(round_robin_len(&runtime), 0);
        let old_identity = super::ChannelIdentity::from_channel(
            &old.channel(
                old.model_rule(ApiFormat::OpenAiChatCompletions, "model")
                    .unwrap()
                    .tiers()[0]
                    .channel_ids()[0],
            )
            .unwrap(),
        );
        let next_identity = super::ChannelIdentity::from_channel(
            &next
                .channel(
                    next.model_rule(ApiFormat::OpenAiChatCompletions, "model")
                        .unwrap()
                        .tiers()[0]
                        .channel_ids()[0],
                )
                .unwrap(),
        );
        assert_ne!(old_identity, next_identity);
        assert!(
            !runtime
                .inner
                .active_channels
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains(&old_identity)
        );

        // An in-flight request can select from its old snapshot, but that stale
        // connectivity identity must not recreate a retained cursor.
        let stale = select(&runtime, &old, &secret);
        assert_eq!(round_robin_len(&runtime), 0);
        drop(stale);

        drop(select(&runtime, &next, &secret));
        assert_eq!(round_robin_len(&runtime), 1);
    }

    #[test]
    fn stale_weighted_round_robin_tiers_cannot_contaminate_new_weights() {
        let (old, secret) = snapshot(
            &[(0, "weighted_round_robin"), (0, "weighted_round_robin")],
            &[1, 1],
        );
        let (next, _) = snapshot(
            &[(0, "weighted_round_robin"), (0, "weighted_round_robin")],
            &[3, 1],
        );
        let runtime = RoutingRuntime::with_seams(
            PassiveHealthPolicy::default(),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(Tickets(Mutex::new(VecDeque::new()))),
        );
        runtime.reconcile(&old);
        drop(select(&runtime, &old, &secret));
        runtime.reconcile(&next);

        // An in-flight request may still use the old tier after publication.
        // Its cursor must be keyed by the old candidate weights.
        drop(select(&runtime, &old, &secret));

        let expected = next
            .model_rule(ApiFormat::OpenAiChatCompletions, "model")
            .unwrap()
            .tiers()[0]
            .channel_ids()[0];
        assert_eq!(select(&runtime, &next, &secret).channel.id(), expected);
        assert_eq!(select(&runtime, &next, &secret).channel.id(), expected);
        assert_eq!(round_robin_len(&runtime), 2);
    }

    #[test]
    fn dropping_a_lease_releases_in_flight_without_changing_health() {
        let (snapshot, secret) = snapshot(&[(0, "weighted_random")], &[1]);
        let runtime = RoutingRuntime::new(PassiveHealthPolicy::default());
        let route = select(&runtime, &snapshot, &secret);
        assert_eq!(runtime.health(&route.channel).in_flight, 1);
        let channel = Arc::clone(&route.channel);
        drop(route);
        let health = runtime.health(&channel);
        assert_eq!(health.in_flight, 0);
        assert_eq!(health.consecutive_connection_failures, 0);
    }
}
