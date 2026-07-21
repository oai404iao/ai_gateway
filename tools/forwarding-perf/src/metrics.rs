//! Fixed-memory latency histograms and serializable load-result metrics.

use serde::{Deserialize, Serialize};

const LINEAR_BUCKETS: usize = 128;
const SUB_BUCKETS: usize = 64;
const EXPONENT_GROUPS: usize = 57;
const HISTOGRAM_BUCKETS: usize = LINEAR_BUCKETS + SUB_BUCKETS * EXPONENT_GROUPS;

#[derive(Clone, Debug)]
pub struct ApproxHistogram {
    buckets: Vec<u64>,
    count: u64,
    sum: u128,
    max: u64,
}

impl Default for ApproxHistogram {
    fn default() -> Self {
        Self {
            buckets: vec![0; HISTOGRAM_BUCKETS],
            count: 0,
            sum: 0,
            max: 0,
        }
    }
}

impl ApproxHistogram {
    pub fn record(&mut self, value: u64) {
        let index = bucket_index(value);
        self.buckets[index] = self.buckets[index].saturating_add(1);
        self.count = self.count.saturating_add(1);
        self.sum = self.sum.saturating_add(u128::from(value));
        self.max = self.max.max(value);
    }

    pub fn merge(&mut self, other: &Self) {
        for (target, source) in self.buckets.iter_mut().zip(&other.buckets) {
            *target = target.saturating_add(*source);
        }
        self.count = self.count.saturating_add(other.count);
        self.sum = self.sum.saturating_add(other.sum);
        self.max = self.max.max(other.max);
    }

    fn quantile(&self, quantile: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let rank = ((self.count as f64 * quantile.clamp(0.0, 1.0)).ceil() as u64).max(1);
        let mut seen = 0_u64;
        for (index, count) in self.buckets.iter().copied().enumerate() {
            seen = seen.saturating_add(count);
            if seen >= rank {
                return bucket_upper_bound(index).min(self.max);
            }
        }
        self.max
    }

    pub fn summary(&self) -> LatencySummary {
        LatencySummary {
            samples: self.count,
            mean_us: if self.count == 0 {
                0.0
            } else {
                self.sum as f64 / self.count as f64
            },
            p50_us: self.quantile(0.50),
            p90_us: self.quantile(0.90),
            p95_us: self.quantile(0.95),
            p99_us: self.quantile(0.99),
            p999_us: self.quantile(0.999),
            max_us: self.max,
        }
    }
}

fn bucket_index(value: u64) -> usize {
    if value < LINEAR_BUCKETS as u64 {
        return value as usize;
    }
    let exponent = 63_u32.saturating_sub(value.leading_zeros());
    let shift = exponent.saturating_sub(6);
    let top = value >> shift;
    let mantissa = top.saturating_sub(64).min(63) as usize;
    let group = exponent.saturating_sub(7) as usize;
    LINEAR_BUCKETS + group * SUB_BUCKETS + mantissa
}

fn bucket_upper_bound(index: usize) -> u64 {
    if index < LINEAR_BUCKETS {
        return index as u64;
    }
    let relative = index - LINEAR_BUCKETS;
    let group = relative / SUB_BUCKETS;
    let mantissa = relative % SUB_BUCKETS;
    let exponent = 7 + group;
    let shift = exponent.saturating_sub(6);
    let top = 64_u128 + mantissa as u128 + 1;
    let exclusive = top.checked_shl(shift as u32).unwrap_or(u128::MAX);
    exclusive.saturating_sub(1).min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Debug, Default)]
pub struct PhaseAccumulator {
    pub requests: u64,
    pub succeeded: u64,
    pub http_errors: u64,
    pub transport_errors: u64,
    pub body_errors: u64,
    pub bytes_received: u64,
    pub latency: ApproxHistogram,
    pub ttft: ApproxHistogram,
}

impl PhaseAccumulator {
    pub fn record(&mut self, observation: RequestObservation) {
        self.requests = self.requests.saturating_add(1);
        self.bytes_received = self
            .bytes_received
            .saturating_add(observation.bytes_received);
        self.latency.record(observation.latency_us);
        if let Some(ttft_us) = observation.ttft_us {
            self.ttft.record(ttft_us);
        }
        match observation.outcome {
            RequestOutcome::Succeeded => {
                self.succeeded = self.succeeded.saturating_add(1);
            }
            RequestOutcome::HttpError => {
                self.http_errors = self.http_errors.saturating_add(1);
            }
            RequestOutcome::TransportError => {
                self.transport_errors = self.transport_errors.saturating_add(1);
            }
            RequestOutcome::BodyError => {
                self.body_errors = self.body_errors.saturating_add(1);
            }
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.succeeded = self.succeeded.saturating_add(other.succeeded);
        self.http_errors = self.http_errors.saturating_add(other.http_errors);
        self.transport_errors = self.transport_errors.saturating_add(other.transport_errors);
        self.body_errors = self.body_errors.saturating_add(other.body_errors);
        self.bytes_received = self.bytes_received.saturating_add(other.bytes_received);
        self.latency.merge(&other.latency);
        self.ttft.merge(&other.ttft);
    }

    pub fn counters(&self) -> LoadCounters {
        LoadCounters {
            requests: self.requests,
            succeeded: self.succeeded,
            http_errors: self.http_errors,
            transport_errors: self.transport_errors,
            body_errors: self.body_errors,
            bytes_received: self.bytes_received,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RequestObservation {
    pub outcome: RequestOutcome,
    pub latency_us: u64,
    pub ttft_us: Option<u64>,
    pub bytes_received: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum RequestOutcome {
    Succeeded,
    HttpError,
    TransportError,
    BodyError,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LatencySummary {
    pub samples: u64,
    pub mean_us: f64,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub max_us: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoadCounters {
    pub requests: u64,
    pub succeeded: u64,
    pub http_errors: u64,
    pub transport_errors: u64,
    pub body_errors: u64,
    pub bytes_received: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoadResult {
    pub scenario: String,
    pub target: String,
    pub api_format: String,
    pub streamed: bool,
    pub concurrency: usize,
    pub warmup_seconds: u64,
    pub duration_seconds: u64,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub warmup: LoadCounters,
    pub measurement: LoadCounters,
    pub achieved_rps: f64,
    pub success_rps: f64,
    pub error_rate: f64,
    pub bytes_per_second: f64,
    pub latency: LatencySummary,
    pub ttft: LatencySummary,
}

impl LoadResult {
    pub fn all_requests(&self) -> u64 {
        self.warmup
            .requests
            .saturating_add(self.measurement.requests)
    }
}

#[cfg(test)]
mod tests {
    use super::{ApproxHistogram, bucket_index, bucket_upper_bound};

    #[test]
    fn histogram_bucket_bounds_cover_recorded_values() {
        for value in [
            0,
            1,
            63,
            127,
            128,
            129,
            255,
            1_000,
            10_000,
            1_000_000,
            u32::MAX as u64,
            u64::MAX,
        ] {
            let index = bucket_index(value);
            assert!(bucket_upper_bound(index) >= value);
        }
    }

    #[test]
    fn histogram_reports_expected_quantile_order() {
        let mut histogram = ApproxHistogram::default();
        for value in 1..=1_000 {
            histogram.record(value);
        }
        let summary = histogram.summary();
        assert!(summary.p50_us <= summary.p90_us);
        assert!(summary.p90_us <= summary.p99_us);
        assert!(summary.p99_us <= summary.max_us);
        assert_eq!(summary.samples, 1_000);
    }
}
