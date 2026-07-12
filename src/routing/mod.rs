//! Priority-aware channel selection and snapshot-independent passive health.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::domain::{
    ApiFormat, CompiledApiKey, CompiledChannel, CompiledModelRule, CompiledRuntimeConfig,
    OutboundNetworkPolicyFingerprint, SelectionStrategy,
};
use uuid::Uuid;

/// Process-wide policy for passive connection health. These values apply to all
/// channels and intentionally do not reuse the currently unsupported per-channel
/// `health_check` control-plane field.
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
struct RuntimeInner {
    policy: PassiveHealthPolicy,
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    state: Mutex<RuntimeState>,
}
#[derive(Default)]
struct RuntimeState {
    channels: HashMap<ChannelIdentity, ChannelState>,
    round_robin: HashMap<RoundRobinKey, HashMap<Uuid, i64>>,
    active_channels: Option<HashSet<ChannelIdentity>>,
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
            connectivity_fingerprint: Arc::from(channel.base_url().as_str()),
            outbound_network_policy_fingerprint: channel
                .upstream_policy()
                .outbound_network_policy_fingerprint(),
        }
    }
}
#[derive(Default)]
struct ChannelState {
    in_flight: u64,
    consecutive_connection_failures: u32,
    cooldown_until: Option<Duration>,
    half_open_probe: bool,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RoundRobinKey {
    rule_id: Uuid,
    priority: i32,
    authorized_candidates: Arc<[Uuid]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelHealthSnapshot {
    pub in_flight: u64,
    pub consecutive_connection_failures: u32,
    pub cooling_down: bool,
    pub half_open_probe: bool,
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
                policy,
                clock,
                entropy,
                state: Mutex::new(RuntimeState::default()),
            }),
        }
    }
    #[must_use]
    pub fn health(&self, channel: &CompiledChannel) -> ChannelHealthSnapshot {
        let now = self.inner.clock.now();
        let state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        let entry = state.channels.get(&ChannelIdentity::from_channel(channel));
        ChannelHealthSnapshot {
            in_flight: entry.map_or(0, |value| value.in_flight),
            consecutive_connection_failures: entry
                .map_or(0, |value| value.consecutive_connection_failures),
            cooling_down: entry
                .and_then(|value| value.cooldown_until)
                .is_some_and(|until| until > now),
            half_open_probe: entry.is_some_and(|value| value.half_open_probe),
        }
    }
    /// Retains runtime health only for channels in the next snapshot, except old
    /// identities which still have an active lease from an earlier snapshot.
    pub fn reconcile(&self, snapshot: &CompiledRuntimeConfig) {
        let active_channels = snapshot
            .channels()
            .map(|channel| ChannelIdentity::from_channel(channel))
            .collect::<HashSet<_>>();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        state.channels.retain(|identity, channel| {
            active_channels.contains(identity) || channel.in_flight > 0
        });
        state.active_channels = Some(active_channels);
        // Cursor state is an optimization, not routing health. Clearing it makes
        // successful reload work proportional to active channels and bounds it to
        // selections made in the current generation.
        state.round_robin.clear();
    }
    #[must_use]
    pub fn select(
        &self,
        snapshot: &CompiledRuntimeConfig,
        key: &CompiledApiKey,
        format: ApiFormat,
        model: &str,
    ) -> SelectionResult {
        let Some(rule) = snapshot.model_rule(format, model) else {
            return SelectionResult::UnknownOrInaccessibleModel;
        };
        let now = self.inner.clock.now();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        let mut has_authorized_candidate = false;
        for tier in rule.tiers() {
            let authorized_candidates = tier
                .channel_ids()
                .iter()
                .filter_map(|id| {
                    let channel = snapshot.channel(*id)?;
                    if key.permits_group(channel.group_id()) {
                        has_authorized_candidate = true;
                        Some(channel)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if !authorized_candidates.is_empty() {
                has_authorized_candidate = true;
            }
            let candidates = authorized_candidates
                .iter()
                .filter(|channel| usable(&mut state, &ChannelIdentity::from_channel(channel), now))
                .cloned()
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }
            let selected_index = match tier.strategy() {
                SelectionStrategy::WeightedRandom => {
                    weighted_ticket(&candidates, &*self.inner.entropy)
                }
                SelectionStrategy::WeightedRoundRobin => {
                    let key = RoundRobinKey {
                        rule_id: rule.id(),
                        priority: tier.priority(),
                        authorized_candidates: Arc::from(
                            authorized_candidates
                                .iter()
                                .map(|channel| channel.id())
                                .collect::<Vec<_>>(),
                        ),
                    };
                    let candidates_are_active =
                        state
                            .active_channels
                            .as_ref()
                            .is_none_or(|active_channels| {
                                authorized_candidates.iter().all(|channel| {
                                    active_channels
                                        .contains(&ChannelIdentity::from_channel(channel))
                                })
                            });
                    if candidates_are_active {
                        smooth_round_robin(state.round_robin.entry(key).or_default(), &candidates)
                    } else {
                        // A request may still hold an old snapshot after reload.
                        // It can route and hold a lease, but cannot grow state for
                        // a retired connectivity identity.
                        smooth_round_robin(&mut HashMap::new(), &candidates)
                    }
                }
            };
            let channel = Arc::clone(&candidates[selected_index]);
            let identity = ChannelIdentity::from_channel(&channel);
            let entry = state.channels.entry(identity.clone()).or_default();
            let half_open_probe = entry.cooldown_until.is_some_and(|until| until <= now);
            if half_open_probe {
                entry.half_open_probe = true;
            }
            entry.in_flight += 1;
            return SelectionResult::Selected(SelectedRoute {
                rule,
                channel,
                lease: ChannelLease {
                    inner: Arc::clone(&self.inner),
                    identity,
                    half_open_probe,
                    released: false,
                },
            });
        }
        if has_authorized_candidate {
            SelectionResult::NoHealthyChannel { rule }
        } else {
            SelectionResult::UnknownOrInaccessibleModel
        }
    }
}

fn usable(state: &mut RuntimeState, identity: &ChannelIdentity, now: Duration) -> bool {
    match state.channels.get(identity) {
        None => true,
        Some(channel) => match channel.cooldown_until {
            None => true,
            Some(until) if until > now => false,
            Some(_) => !channel.half_open_probe,
        },
    }
}

fn weighted_ticket(channels: &[Arc<CompiledChannel>], entropy: &dyn Entropy) -> usize {
    let total = channels
        .iter()
        .map(|channel| u64::try_from(channel.weight()).expect("compiled positive weight"))
        .sum::<u64>();
    let zone = u64::MAX - (u64::MAX % total);
    let ticket = loop {
        let value = entropy.next_u64();
        if value < zone {
            break value % total;
        }
    };
    let mut remaining = ticket;
    for (index, channel) in channels.iter().enumerate() {
        let weight = u64::try_from(channel.weight()).expect("compiled positive weight");
        if remaining < weight {
            return index;
        }
        remaining -= weight;
    }
    unreachable!("ticket is bounded by compiled total weight")
}

fn smooth_round_robin(
    current: &mut HashMap<Uuid, i64>,
    channels: &[Arc<CompiledChannel>],
) -> usize {
    let total = channels
        .iter()
        .map(|channel| i64::from(channel.weight()))
        .sum::<i64>();
    let allowed = channels
        .iter()
        .map(|channel| channel.id())
        .collect::<HashSet<_>>();
    current.retain(|id, _| allowed.contains(id));
    let mut winner = 0;
    for (index, channel) in channels.iter().enumerate() {
        let value = current.entry(channel.id()).or_insert(0);
        *value += i64::from(channel.weight());
        if *value
            > *current
                .get(&channels[winner].id())
                .expect("current weight exists")
        {
            winner = index;
        }
    }
    *current
        .get_mut(&channels[winner].id())
        .expect("winner exists") -= total;
    winner
}

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
    pub lease: ChannelLease,
}

/// Releases in-flight accounting when the selected response completes or is cancelled.
/// It is deliberately non-cloneable so every selection has exactly one lease owner.
pub struct ChannelLease {
    inner: Arc<RuntimeInner>,
    identity: ChannelIdentity,
    half_open_probe: bool,
    released: bool,
}
impl ChannelLease {
    pub fn response_headers_received(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        let entry = state.channels.entry(self.identity.clone()).or_default();
        entry.consecutive_connection_failures = 0;
        entry.cooldown_until = None;
        entry.half_open_probe = false;
    }
    pub fn connection_failed(&mut self) {
        let now = self.inner.clock.now();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        let entry = state.channels.entry(self.identity.clone()).or_default();
        entry.consecutive_connection_failures =
            entry.consecutive_connection_failures.saturating_add(1);
        if entry.consecutive_connection_failures >= self.inner.policy.connection_failure_threshold {
            entry.cooldown_until = Some(now + self.inner.policy.cooldown);
            entry.half_open_probe = false;
        }
    }
    /// A half-open request reached neither response headers nor a known-success
    /// state. Reopen the cooldown without treating ordinary header timeouts as
    /// connection failures.
    pub fn probe_failed(&mut self) {
        if !self.half_open_probe {
            return;
        }
        let now = self.inner.clock.now();
        let mut state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        let entry = state.channels.entry(self.identity.clone()).or_default();
        if entry.half_open_probe {
            entry.cooldown_until = Some(now + self.inner.policy.cooldown);
            entry.half_open_probe = false;
        }
    }
}
impl Drop for ChannelLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .expect("routing state mutex poisoned");
        let remove_retired = if let Some(entry) = state.channels.get_mut(&self.identity) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
            // Cancellation is neutral. Explicit failed-probe transitions reopen cooldown.
            if self.half_open_probe && entry.half_open_probe {
                entry.half_open_probe = false;
            }
            entry.in_flight == 0
                && state
                    .active_channels
                    .as_ref()
                    .is_some_and(|channels| !channels.contains(&self.identity))
        } else {
            false
        };
        if remove_retired {
            state.channels.remove(&self.identity);
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
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    use crate::{
        domain::{ApiFormat, CompiledRuntimeConfig},
        persistence::{
            ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
            ProxyRecord,
        },
        runtime_config::compile_control_plane,
    };
    use uuid::Uuid;

    use super::{Clock, Entropy, PassiveHealthPolicy, RoutingRuntime, SelectionResult};

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
                id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                user_status: "active".into(),
                secret_value: secret.clone(),
                status: "active".into(),
                expires_at: None,
                allowed_api_formats: vec!["open_ai_chat_completions".into()],
                permissions: vec!["proxy".into()],
                allowed_group_ids: None,
                requests_per_minute: None,
                tokens_per_minute: None,
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
                    auto_disabled: false,
                    weight: *weight,
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
                    health_check: serde_json::json!({}),
                })
                .collect(),
            model_rules: vec![ModelRuleRecord {
                id: Uuid::new_v4(),
                client_model: "model".into(),
                api_format: "open_ai_chat_completions".into(),
                model_id: Uuid::new_v4(),
                model_enabled: true,
                upstream_model: "upstream".into(),
                channel_group_ids: group_ids,
                channel_ids: vec![],
                enabled: true,
            }],
            proxies: vec![],
            templates: vec![],
        };
        (compile_control_plane(records).unwrap(), secret)
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
                secret_value: secret.clone(),
                status: "active".into(),
                expires_at: None,
                allowed_api_formats: vec!["open_ai_chat_completions".into()],
                permissions: vec!["proxy".into()],
                allowed_group_ids: None,
                requests_per_minute: None,
                tokens_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
                quota_used_amount: Default::default(),
            }],
            groups: vec![ChannelGroupRecord {
                id: group_id,
                name: "group".into(),
                api_format: "open_ai_chat_completions".into(),
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
                auto_disabled: false,
                weight: 1,
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
                health_check: serde_json::json!({}),
            }],
            model_rules: vec![ModelRuleRecord {
                id: Uuid::from_u128(400),
                client_model: "model".into(),
                api_format: "open_ai_chat_completions".into(),
                model_id: Uuid::from_u128(401),
                model_enabled: true,
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

        let state = runtime.inner.state.lock().unwrap();
        assert_eq!(state.round_robin.len(), 1);
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
        assert_eq!(runtime.inner.state.lock().unwrap().round_robin.len(), 1);

        runtime.reconcile(&next);
        assert!(runtime.inner.state.lock().unwrap().round_robin.is_empty());
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
                .state
                .lock()
                .unwrap()
                .active_channels
                .as_ref()
                .unwrap()
                .contains(&old_identity)
        );

        // An in-flight request can select from its old snapshot, but that stale
        // connectivity identity must not recreate a retained cursor.
        let stale = select(&runtime, &old, &secret);
        assert!(runtime.inner.state.lock().unwrap().round_robin.is_empty());
        drop(stale);

        drop(select(&runtime, &next, &secret));
        assert_eq!(runtime.inner.state.lock().unwrap().round_robin.len(), 1);
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
