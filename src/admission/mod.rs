//! Process-local request admission state kept independently from configuration snapshots.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::domain::CompiledApiKey;

const WINDOW: Duration = Duration::from_secs(60);

/// Monotonic time source, injectable so admission boundaries are deterministic in tests.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Duration;
}

#[derive(Debug)]
struct SystemClock {
    started: Instant,
}
impl SystemClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
}

#[derive(Debug, Default)]
struct KeyState {
    window_started_at: Option<Duration>,
    requests_in_window: u32,
    in_flight: u32,
}

#[derive(Default)]
struct AdmissionState {
    keys: HashMap<Uuid, KeyState>,
    settled_quota_used: HashMap<Uuid, Decimal>,
}

struct AdmissionInner {
    clock: Arc<dyn Clock>,
    state: Mutex<AdmissionState>,
}

/// Admission runtime shared by every request in a gateway process. It never
/// reconciles snapshot contents: state is deliberately retained by API-key UUID.
#[derive(Clone)]
pub struct AdmissionRuntime {
    inner: Arc<AdmissionInner>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AdmissionPressureSnapshot {
    pub tracked_api_keys: u64,
    pub requests_in_current_windows: u64,
    pub in_flight_requests: u64,
}

impl Default for AdmissionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl AdmissionRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock::new()))
    }

    #[must_use]
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            inner: Arc::new(AdmissionInner {
                clock,
                state: Mutex::new(AdmissionState::default()),
            }),
        }
    }

    /// Applies the settled-usage soft quota precheck before RPM and concurrency.
    ///
    /// A quota only rejects when settled usage is already at or above its
    /// configured limit. This runtime never estimates, reserves, releases, or
    /// settles money, so an under-limit request is allowed even if it may cost
    /// more than the remaining amount.
    pub fn admit(&self, key: &CompiledApiKey) -> Result<AdmissionLease, AdmissionError> {
        let mut all = self
            .inner
            .state
            .lock()
            .expect("admission state mutex poisoned");
        let now = self.inner.clock.now();
        all.keys.retain(|_, state| {
            state.in_flight > 0
                || state
                    .window_started_at
                    .is_some_and(|started| now.saturating_sub(started) <= WINDOW)
        });
        let settled_quota_used = all.settled_quota_used.get(&key.id()).copied().map_or_else(
            || key.quota_used_amount(),
            |used| used.max(key.quota_used_amount()),
        );
        if key.quota_exhausted_at(settled_quota_used) {
            return Err(AdmissionError::InsufficientQuota);
        }
        let state = all.keys.entry(key.id()).or_default();
        if state
            .window_started_at
            .is_none_or(|started| now.saturating_sub(started) >= WINDOW)
        {
            state.window_started_at = Some(now);
            state.requests_in_window = 0;
        }

        if let Some(limit) = key.requests_per_minute() {
            if state.requests_in_window >= limit {
                let reset_at = state.window_started_at.expect("window initialized") + WINDOW;
                return Err(AdmissionError::RateLimited {
                    retry_after: retry_after(now, reset_at),
                });
            }
        }
        // A request that reaches this point has passed the quota gate and made
        // an admission decision. Concurrency rejection intentionally consumes
        // this RPM slot; rate rejection does not increment beyond the limit.
        state.requests_in_window = state.requests_in_window.saturating_add(1);

        if key
            .max_concurrent_requests()
            .is_some_and(|limit| state.in_flight >= limit)
        {
            return Err(AdmissionError::ConcurrentLimited);
        }
        // Count unrestricted requests too, so a new restrictive snapshot sees
        // work admitted under a prior unlimited policy.
        state.in_flight = state.in_flight.saturating_add(1);
        Ok(AdmissionLease {
            inner: Arc::clone(&self.inner),
            key_id: key.id(),
            released: false,
        })
    }

    /// Publishes the database's authoritative total after a successful
    /// settlement. It only moves upward because management APIs do not own
    /// `quota_used_amount`; a later snapshot reload remains the cross-process
    /// source of truth.
    pub fn record_settled_quota_usage(&self, key_id: Uuid, quota_used_amount: Decimal) {
        let mut all = self
            .inner
            .state
            .lock()
            .expect("admission state mutex poisoned");
        all.settled_quota_used
            .entry(key_id)
            .and_modify(|current| *current = (*current).max(quota_used_amount))
            .or_insert(quota_used_amount);
    }

    #[must_use]
    pub fn pressure_snapshot(&self) -> AdmissionPressureSnapshot {
        let all = self
            .inner
            .state
            .lock()
            .expect("admission state mutex poisoned");
        AdmissionPressureSnapshot {
            tracked_api_keys: u64::try_from(all.keys.len()).unwrap_or(u64::MAX),
            requests_in_current_windows: all.keys.values().fold(0_u64, |total, state| {
                total.saturating_add(u64::from(state.requests_in_window))
            }),
            in_flight_requests: all.keys.values().fold(0_u64, |total, state| {
                total.saturating_add(u64::from(state.in_flight))
            }),
        }
    }

    #[cfg(test)]
    fn state_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("admission state mutex poisoned")
            .keys
            .len()
    }
}

fn retry_after(now: Duration, reset_at: Duration) -> u64 {
    let remaining = reset_at.saturating_sub(now);
    let seconds = remaining.as_secs();
    seconds + u64::from(remaining.subsec_nanos() != 0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    RateLimited { retry_after: u64 },
    ConcurrentLimited,
    InsufficientQuota,
}

/// Non-cloneable guard which decrements in-flight state exactly once on drop.
pub struct AdmissionLease {
    inner: Arc<AdmissionInner>,
    key_id: Uuid,
    released: bool,
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut all = self
            .inner
            .state
            .lock()
            .expect("admission state mutex poisoned");
        let remove = {
            let state = all.keys.get_mut(&self.key_id).expect("lease state exists");
            state.in_flight = state.in_flight.saturating_sub(1);
            state.in_flight == 0
                && state
                    .window_started_at
                    .is_some_and(|started| self.inner.clock.now().saturating_sub(started) > WINDOW)
        };
        // A state can only be removed after every lease is gone and its RPM
        // window has expired. The same mutex makes this safe against a newer
        // admission: it either resets the window first or creates fresh state
        // after this removal.
        if remove {
            all.keys.remove(&self.key_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rust_decimal::Decimal;
    use uuid::Uuid;

    use super::{AdmissionError, AdmissionRuntime, Clock};
    use crate::domain::CompiledApiKey;

    #[derive(Default)]
    struct TestClock(Mutex<std::time::Duration>);
    impl TestClock {
        fn advance(&self, duration: std::time::Duration) {
            *self.0.lock().unwrap() += duration;
        }
    }
    impl Clock for TestClock {
        fn now(&self) -> std::time::Duration {
            *self.0.lock().unwrap()
        }
    }

    fn key(id: Uuid, rpm: Option<u32>, concurrent: Option<u32>) -> CompiledApiKey {
        CompiledApiKey::test_with_policy(
            id,
            rpm,
            concurrent,
            Some(Decimal::new(10, 0)),
            Decimal::ZERO,
        )
    }

    #[test]
    fn rpm_is_precise_per_key_and_resets_at_window_boundary() {
        let clock = Arc::new(TestClock::default());
        let runtime = AdmissionRuntime::with_clock(clock.clone());
        let first = key(Uuid::new_v4(), Some(2), None);
        let other = key(Uuid::new_v4(), Some(1), None);
        let a = runtime.admit(&first).unwrap();
        drop(a);
        let b = runtime.admit(&first).unwrap();
        drop(b);
        assert!(matches!(
            runtime.admit(&first),
            Err(AdmissionError::RateLimited { retry_after: 60 })
        ));
        let other_lease = runtime.admit(&other).unwrap();
        drop(other_lease);
        clock.advance(std::time::Duration::from_secs(59));
        assert!(matches!(
            runtime.admit(&first),
            Err(AdmissionError::RateLimited { retry_after: 1 })
        ));
        clock.advance(std::time::Duration::from_secs(1));
        drop(runtime.admit(&first).unwrap());
    }

    #[test]
    fn concurrency_release_and_denial_consumes_rpm_without_underflow() {
        let runtime = AdmissionRuntime::new();
        let key = key(Uuid::new_v4(), Some(2), Some(1));
        let lease = runtime.admit(&key).unwrap();
        assert!(matches!(
            runtime.admit(&key),
            Err(AdmissionError::ConcurrentLimited)
        ));
        drop(lease);
        assert!(matches!(
            runtime.admit(&key),
            Err(AdmissionError::RateLimited { retry_after: 60 })
        ));
    }

    #[test]
    fn old_unlimited_lease_counts_after_policy_becomes_restrictive() {
        let runtime = AdmissionRuntime::new();
        let id = Uuid::new_v4();
        let unlimited = key(id, None, None);
        let lease = runtime.admit(&unlimited).unwrap();
        let restrictive = key(id, None, Some(1));
        assert!(matches!(
            runtime.admit(&restrictive),
            Err(AdmissionError::ConcurrentLimited)
        ));
        drop(lease);
        drop(runtime.admit(&restrictive).unwrap());
    }

    #[test]
    fn quota_equality_is_denied_without_creating_a_lease() {
        let runtime = AdmissionRuntime::new();
        let key = CompiledApiKey::test_with_policy(
            Uuid::new_v4(),
            None,
            Some(1),
            Some(Decimal::new(100, 2)),
            Decimal::new(100, 2),
        );
        assert!(matches!(
            runtime.admit(&key),
            Err(AdmissionError::InsufficientQuota)
        ));
    }

    #[test]
    fn freshly_settled_quota_is_enforced_before_snapshot_reload() {
        let runtime = AdmissionRuntime::new();
        let id = Uuid::new_v4();
        let key = CompiledApiKey::test_with_policy(
            id,
            None,
            None,
            Some(Decimal::new(100, 2)),
            Decimal::ZERO,
        );
        runtime.record_settled_quota_usage(id, Decimal::new(100, 2));
        assert!(matches!(
            runtime.admit(&key),
            Err(AdmissionError::InsufficientQuota)
        ));
    }

    #[test]
    fn idle_state_is_retained_at_the_window_boundary() {
        let clock = Arc::new(TestClock::default());
        let runtime = AdmissionRuntime::with_clock(clock.clone());
        let key = key(Uuid::new_v4(), Some(1), Some(1));
        let lease = runtime.admit(&key).unwrap();
        clock.advance(std::time::Duration::from_secs(60));
        assert_eq!(runtime.state_count(), 1);
        drop(lease);
        assert_eq!(runtime.state_count(), 1);
        clock.advance(std::time::Duration::from_secs(1));
        drop(runtime.admit(&key).unwrap());
        assert_eq!(runtime.state_count(), 1);
    }

    #[test]
    fn later_admission_prunes_expired_idle_states() {
        let clock = Arc::new(TestClock::default());
        let runtime = AdmissionRuntime::with_clock(clock.clone());
        let old = key(Uuid::new_v4(), Some(1), None);
        let current = key(Uuid::new_v4(), Some(1), None);

        drop(runtime.admit(&old).unwrap());
        assert_eq!(runtime.state_count(), 1);
        clock.advance(std::time::Duration::from_secs(61));

        drop(runtime.admit(&current).unwrap());
        assert_eq!(runtime.state_count(), 1);
    }
}
