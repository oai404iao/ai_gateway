//! Current-instance resource, runtime-pressure, queue, and backlog snapshots.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::{
    admission::{AdmissionPressureSnapshot, AdmissionRuntime},
    routing::{RoutingPressureSnapshot, RoutingRuntime},
};

use super::{
    channel_automation::AutomaticDisableService,
    proxy::ProxyService,
    request_log::{RequestLogPipelineMonitor, RequestLogPipelineSnapshot},
};

const MIN_RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct SystemMetricsService {
    sampler: Arc<Mutex<ResourceSampler>>,
    started_at: DateTime<Utc>,
    started: Instant,
    control_plane_pool: PgPool,
    control_plane_pool_capacity: u32,
    admission: Option<AdmissionRuntime>,
    routing: Option<RoutingRuntime>,
    request_logs: Option<RequestLogPipelineMonitor>,
    automatic_disable: Option<AutomaticDisableService>,
    websocket_proxy: Option<ProxyService>,
}

impl SystemMetricsService {
    #[must_use]
    pub fn new(control_plane_pool: PgPool, control_plane_pool_capacity: u32) -> Self {
        Self::new_at(
            control_plane_pool,
            control_plane_pool_capacity,
            Utc::now(),
            Instant::now(),
        )
    }

    #[must_use]
    pub fn new_at(
        control_plane_pool: PgPool,
        control_plane_pool_capacity: u32,
        started_at: DateTime<Utc>,
        started: Instant,
    ) -> Self {
        Self {
            sampler: Arc::new(Mutex::new(ResourceSampler::new())),
            started_at,
            started,
            control_plane_pool,
            control_plane_pool_capacity,
            admission: None,
            routing: None,
            request_logs: None,
            automatic_disable: None,
            websocket_proxy: None,
        }
    }

    #[must_use]
    pub fn with_runtime(
        mut self,
        admission: AdmissionRuntime,
        routing: RoutingRuntime,
        request_logs: RequestLogPipelineMonitor,
        automatic_disable: AutomaticDisableService,
    ) -> Self {
        self.admission = Some(admission);
        self.routing = Some(routing);
        self.request_logs = Some(request_logs);
        self.automatic_disable = Some(automatic_disable);
        self
    }

    #[must_use]
    pub fn with_websocket_proxy(mut self, proxy: ProxyService) -> Self {
        self.websocket_proxy = Some(proxy);
        self
    }

    pub async fn snapshot(&self) -> SystemLoadReport {
        let sampler = Arc::clone(&self.sampler);
        let resources = tokio::task::spawn_blocking(move || {
            sampler
                .lock()
                .map(|mut sampler| sampler.sample())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        let admission = self
            .admission
            .as_ref()
            .map_or_else(AdmissionPressureSnapshot::default, |runtime| {
                runtime.pressure_snapshot()
            });
        let routing = self
            .routing
            .as_ref()
            .map_or_else(RoutingPressureSnapshot::default, |runtime| {
                runtime.pressure_snapshot()
            });
        let request_logs = match &self.request_logs {
            Some(monitor) => monitor.snapshot().await,
            None => RequestLogPipelineSnapshot::default(),
        };
        let automatic_disable_queue =
            self.automatic_disable
                .as_ref()
                .map_or_else(SystemQueueLoad::default, |service| {
                    let (depth, capacity) = service.queue_depth_and_capacity();
                    SystemQueueLoad::new(depth, capacity)
                });
        let control_plane_pool = database_pool_load(
            u64::from(self.control_plane_pool.size()),
            usize_to_u64(self.control_plane_pool.num_idle()),
            u64::from(self.control_plane_pool_capacity),
        );
        let request_log_pool = database_pool_load(
            request_logs.database_pool_size,
            request_logs.database_pool_idle,
            request_logs.database_pool_capacity,
        );
        let websocket = self
            .websocket_proxy
            .as_ref()
            .map_or_else(Default::default, ProxyService::websocket_runtime_snapshot);

        SystemLoadReport {
            sampled_at: Utc::now(),
            started_at: self.started_at,
            uptime_seconds: self.started.elapsed().as_secs(),
            host: SystemHostLoad {
                logical_cpu_count: resources.logical_cpu_count,
                cpu_usage_percent: resources.host_cpu_usage_percent,
                load_average_1m: resources.load_average_1m,
                load_average_5m: resources.load_average_5m,
                load_average_15m: resources.load_average_15m,
                memory_total_bytes: resources.host_memory_total_bytes,
                memory_used_bytes: resources.host_memory_used_bytes,
                memory_usage_percent: percentage(
                    resources.host_memory_used_bytes,
                    resources.host_memory_total_bytes,
                ),
            },
            process: SystemProcessLoad {
                cpu_usage_percent: resources.process_cpu_usage_percent,
                resident_memory_bytes: resources.process_resident_memory_bytes,
                resident_memory_percent: percentage(
                    resources.process_resident_memory_bytes,
                    resources.host_memory_total_bytes,
                ),
                open_file_descriptors: resources.open_file_descriptors,
                threads: resources.threads,
            },
            runtime: SystemRuntimeLoad {
                tracked_api_keys: admission.tracked_api_keys,
                requests_in_current_windows: admission.requests_in_current_windows,
                in_flight_requests: admission.in_flight_requests,
                routing_in_flight_requests: routing.in_flight_requests,
                tracked_channels: routing.tracked_channels,
                cooling_down_channels: routing.cooling_down_channels,
                half_open_channels: routing.half_open_channels,
                session_affinity_entries: routing.session_affinity_entries,
            },
            queues: SystemQueuesLoad {
                request_log_notifications: SystemQueueLoad::new(
                    request_logs.notification_queue_depth,
                    request_logs.notification_queue_capacity,
                ),
                request_log_projection: SystemQueueLoad::new(
                    request_logs.projection_queue_depth,
                    request_logs.projection_queue_capacity,
                ),
                automatic_disable: automatic_disable_queue,
            },
            request_log: SystemRequestLogLoad {
                spool_pending_bytes: request_logs.spool_pending_bytes,
                ingress_backlog_rows_estimate: request_logs.ingress_backlog_rows_estimate,
                ingress_oldest_age_seconds: request_logs.ingress_oldest_age_seconds,
                settlement_backlog_rows: request_logs.settlement_backlog_rows,
                settlement_oldest_age_seconds: request_logs.settlement_oldest_age_seconds,
                recorded_total: request_logs.recorded_total,
                spooled_total: request_logs.spooled_total,
                projected_rows_total: request_logs.projected_rows_total,
                projection_deferred_total: request_logs.projection_deferred_total,
                settled_rows_total: request_logs.settled_rows_total,
                spool_append_failures_total: request_logs.spool_append_failures_total,
                ingress_failures_total: request_logs.ingress_failures_total,
                projection_failures_total: request_logs.projection_failures_total,
                settlement_failures_total: request_logs.settlement_failures_total,
            },
            websocket: SystemWebSocketLoad {
                enabled: websocket.enabled,
                active_downstream_sessions: websocket.active_downstream_sessions,
                idle_upstream_connections: websocket.idle_upstream_connections,
                leased_upstream_connections: websocket.leased_upstream_connections,
                pool_capacity: websocket.pool_capacity,
                idle_pool_utilization_percent: percentage(
                    Some(websocket.idle_upstream_connections),
                    Some(websocket.pool_capacity),
                ),
                pool_hits_total: websocket.pool_hits_total,
                pool_misses_total: websocket.pool_misses_total,
                pool_discarded_total: websocket.pool_discarded_total,
                idle_timeout_seconds: websocket.idle_timeout_seconds,
                max_connection_age_seconds: websocket.max_connection_age_seconds,
            },
            database: SystemDatabaseLoad {
                control_plane: control_plane_pool,
                request_log: request_log_pool,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemLoadReport {
    pub sampled_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub uptime_seconds: u64,
    pub host: SystemHostLoad,
    pub process: SystemProcessLoad,
    pub runtime: SystemRuntimeLoad,
    pub queues: SystemQueuesLoad,
    pub request_log: SystemRequestLogLoad,
    pub websocket: SystemWebSocketLoad,
    pub database: SystemDatabaseLoad,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemHostLoad {
    pub logical_cpu_count: u64,
    pub cpu_usage_percent: Option<f64>,
    pub load_average_1m: Option<f64>,
    pub load_average_5m: Option<f64>,
    pub load_average_15m: Option<f64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_usage_percent: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemProcessLoad {
    pub cpu_usage_percent: Option<f64>,
    pub resident_memory_bytes: Option<u64>,
    pub resident_memory_percent: Option<f64>,
    pub open_file_descriptors: Option<u64>,
    pub threads: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemRuntimeLoad {
    pub tracked_api_keys: u64,
    pub requests_in_current_windows: u64,
    pub in_flight_requests: u64,
    pub routing_in_flight_requests: u64,
    pub tracked_channels: u64,
    pub cooling_down_channels: u64,
    pub half_open_channels: u64,
    pub session_affinity_entries: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemQueuesLoad {
    pub request_log_notifications: SystemQueueLoad,
    pub request_log_projection: SystemQueueLoad,
    pub automatic_disable: SystemQueueLoad,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SystemQueueLoad {
    pub depth: u64,
    pub capacity: u64,
    pub utilization_percent: Option<f64>,
}

impl SystemQueueLoad {
    fn new(depth: u64, capacity: u64) -> Self {
        Self {
            depth,
            capacity,
            utilization_percent: if capacity == 0 {
                None
            } else {
                Some((depth.min(capacity) as f64 / capacity as f64) * 100.0)
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemRequestLogLoad {
    pub spool_pending_bytes: u64,
    pub ingress_backlog_rows_estimate: Option<u64>,
    pub ingress_oldest_age_seconds: Option<u64>,
    pub settlement_backlog_rows: Option<u64>,
    pub settlement_oldest_age_seconds: Option<u64>,
    pub recorded_total: u64,
    pub spooled_total: u64,
    pub projected_rows_total: u64,
    pub projection_deferred_total: u64,
    pub settled_rows_total: u64,
    pub spool_append_failures_total: u64,
    pub ingress_failures_total: u64,
    pub projection_failures_total: u64,
    pub settlement_failures_total: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemWebSocketLoad {
    pub enabled: bool,
    pub active_downstream_sessions: u64,
    pub idle_upstream_connections: u64,
    pub leased_upstream_connections: u64,
    pub pool_capacity: u64,
    pub idle_pool_utilization_percent: Option<f64>,
    pub pool_hits_total: u64,
    pub pool_misses_total: u64,
    pub pool_discarded_total: u64,
    pub idle_timeout_seconds: u64,
    pub max_connection_age_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemDatabaseLoad {
    pub control_plane: SystemDatabasePoolLoad,
    pub request_log: SystemDatabasePoolLoad,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemDatabasePoolLoad {
    pub size: u64,
    pub idle: u64,
    pub in_use: u64,
    pub capacity: u64,
    pub utilization_percent: Option<f64>,
}

fn database_pool_load(size: u64, idle: u64, capacity: u64) -> SystemDatabasePoolLoad {
    let in_use = size.saturating_sub(idle);
    SystemDatabasePoolLoad {
        size,
        idle,
        in_use,
        capacity,
        utilization_percent: if capacity == 0 {
            None
        } else {
            Some((in_use.min(capacity) as f64 / capacity as f64) * 100.0)
        },
    }
}

fn percentage(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            Some((used.min(total) as f64 / total as f64) * 100.0)
        }
        _ => None,
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, Default)]
struct ResourceSnapshot {
    logical_cpu_count: u64,
    host_cpu_usage_percent: Option<f64>,
    process_cpu_usage_percent: Option<f64>,
    load_average_1m: Option<f64>,
    load_average_5m: Option<f64>,
    load_average_15m: Option<f64>,
    host_memory_total_bytes: Option<u64>,
    host_memory_used_bytes: Option<u64>,
    process_resident_memory_bytes: Option<u64>,
    open_file_descriptors: Option<u64>,
    threads: Option<u64>,
}

struct ResourceSampler {
    previous: RawResourceSample,
    latest: ResourceSnapshot,
    last_sampled: Instant,
}

impl ResourceSampler {
    fn new() -> Self {
        let previous = read_resource_sample();
        let latest = resource_snapshot(&previous, None);
        Self {
            previous,
            latest,
            last_sampled: Instant::now(),
        }
    }

    fn sample(&mut self) -> ResourceSnapshot {
        if self.last_sampled.elapsed() < MIN_RESOURCE_SAMPLE_INTERVAL {
            return self.latest;
        }
        let current = read_resource_sample();
        self.latest = resource_snapshot(&current, Some(&self.previous));
        self.previous = current;
        self.last_sampled = Instant::now();
        self.latest
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct RawResourceSample {
    logical_cpu_count: u64,
    cpu: Option<CpuTimes>,
    process_cpu_ticks: Option<u64>,
    load_average: Option<(f64, f64, f64)>,
    host_memory_total_bytes: Option<u64>,
    host_memory_available_bytes: Option<u64>,
    process_resident_memory_bytes: Option<u64>,
    open_file_descriptors: Option<u64>,
    threads: Option<u64>,
}

fn resource_snapshot(
    current: &RawResourceSample,
    previous: Option<&RawResourceSample>,
) -> ResourceSnapshot {
    let host_cpu_usage_percent =
        previous.and_then(|previous| cpu_usage_percent(previous.cpu?, current.cpu?));
    let process_cpu_usage_percent = previous.and_then(|previous| {
        process_cpu_usage_percent(
            previous.cpu?,
            current.cpu?,
            previous.process_cpu_ticks?,
            current.process_cpu_ticks?,
        )
    });
    let host_memory_used_bytes = match (
        current.host_memory_total_bytes,
        current.host_memory_available_bytes,
    ) {
        (Some(total), Some(available)) => Some(total.saturating_sub(available.min(total))),
        _ => None,
    };
    let (load_average_1m, load_average_5m, load_average_15m) =
        current.load_average.map_or((None, None, None), |load| {
            (Some(load.0), Some(load.1), Some(load.2))
        });
    ResourceSnapshot {
        logical_cpu_count: current.logical_cpu_count,
        host_cpu_usage_percent,
        process_cpu_usage_percent,
        load_average_1m,
        load_average_5m,
        load_average_15m,
        host_memory_total_bytes: current.host_memory_total_bytes,
        host_memory_used_bytes,
        process_resident_memory_bytes: current.process_resident_memory_bytes,
        open_file_descriptors: current.open_file_descriptors,
        threads: current.threads,
    }
}

fn cpu_usage_percent(previous: CpuTimes, current: CpuTimes) -> Option<f64> {
    let total = current.total.checked_sub(previous.total)?;
    if total == 0 {
        return None;
    }
    let idle = current.idle.saturating_sub(previous.idle).min(total);
    Some(((total - idle) as f64 / total as f64) * 100.0)
}

fn process_cpu_usage_percent(
    previous_cpu: CpuTimes,
    current_cpu: CpuTimes,
    previous_process_ticks: u64,
    current_process_ticks: u64,
) -> Option<f64> {
    let total = current_cpu.total.checked_sub(previous_cpu.total)?;
    if total == 0 {
        return None;
    }
    let process = current_process_ticks
        .checked_sub(previous_process_ticks)?
        .min(total);
    Some((process as f64 / total as f64) * 100.0)
}

#[cfg(target_os = "linux")]
fn read_resource_sample() -> RawResourceSample {
    let cpu = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|content| parse_cpu_times(&content));
    let process_cpu_ticks = std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|content| parse_process_cpu_ticks(&content));
    let (host_memory_total_bytes, host_memory_available_bytes) =
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|content| parse_memory(&content))
            .map_or((None, None), |memory| (Some(memory.0), Some(memory.1)));
    let (process_resident_memory_bytes, threads) = std::fs::read_to_string("/proc/self/status")
        .ok()
        .map(|content| parse_process_status(&content))
        .unwrap_or((None, None));
    RawResourceSample {
        logical_cpu_count: std::thread::available_parallelism()
            .map(|count| usize_to_u64(count.get()))
            .unwrap_or(1),
        cpu,
        process_cpu_ticks,
        load_average: std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|content| parse_load_average(&content)),
        host_memory_total_bytes,
        host_memory_available_bytes,
        process_resident_memory_bytes,
        open_file_descriptors: std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|entries| usize_to_u64(entries.count())),
        threads,
    }
}

#[cfg(not(target_os = "linux"))]
fn read_resource_sample() -> RawResourceSample {
    RawResourceSample {
        logical_cpu_count: std::thread::available_parallelism()
            .map(|count| usize_to_u64(count.get()))
            .unwrap_or(1),
        ..RawResourceSample::default()
    }
}

fn parse_cpu_times(content: &str) -> Option<CpuTimes> {
    let line = content.lines().find(|line| line.starts_with("cpu "))?;
    let values = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let total = values.iter().copied().try_fold(0_u64, u64::checked_add)?;
    let idle = values[3].checked_add(values.get(4).copied().unwrap_or(0))?;
    Some(CpuTimes { total, idle })
}

fn parse_process_cpu_ticks(content: &str) -> Option<u64> {
    let command_end = content.rfind(')')?;
    let fields = content
        .get(command_end + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let user_ticks = fields.get(11)?.parse::<u64>().ok()?;
    let system_ticks = fields.get(12)?.parse::<u64>().ok()?;
    user_ticks.checked_add(system_ticks)
}

fn parse_memory(content: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    let mut free = None;
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        match fields.next()? {
            "MemTotal:" => total = parse_kib(fields.next()?),
            "MemAvailable:" => available = parse_kib(fields.next()?),
            "MemFree:" => free = parse_kib(fields.next()?),
            _ => {}
        }
    }
    Some((total?, available.or(free)?))
}

fn parse_process_status(content: &str) -> (Option<u64>, Option<u64>) {
    let mut resident = None;
    let mut threads = None;
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("VmRSS:") => resident = fields.next().and_then(parse_kib),
            Some("Threads:") => threads = fields.next().and_then(|value| value.parse().ok()),
            _ => {}
        }
    }
    (resident, threads)
}

fn parse_kib(value: &str) -> Option<u64> {
    value.parse::<u64>().ok()?.checked_mul(1_024)
}

fn parse_load_average(content: &str) -> Option<(f64, f64, f64)> {
    let mut fields = content.split_whitespace();
    Some((
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        CpuTimes, SystemQueueLoad, cpu_usage_percent, parse_cpu_times, parse_load_average,
        parse_memory, parse_process_cpu_ticks, parse_process_status, process_cpu_usage_percent,
    };

    #[test]
    fn parses_linux_proc_samples() {
        assert_eq!(
            parse_cpu_times("cpu  10 2 3 20 5 1 1 0 0 0\ncpu0 0 0 0 0"),
            Some(CpuTimes {
                total: 42,
                idle: 25
            })
        );
        assert_eq!(
            parse_memory("MemTotal: 1000 kB\nMemFree: 200 kB\nMemAvailable: 750 kB\n"),
            Some((1_024_000, 768_000))
        );
        assert_eq!(
            parse_process_status("Name:\ttest\nVmRSS:\t123 kB\nThreads:\t7\n"),
            (Some(125_952), Some(7))
        );
        assert_eq!(
            parse_load_average("0.12 0.34 0.56 1/100 1"),
            Some((0.12, 0.34, 0.56))
        );
    }

    #[test]
    fn parses_process_stat_after_parenthesized_command() {
        let stat = "1 (gateway worker) S 0 0 0 0 0 0 0 0 0 0 30 12 0 0 0 0 1 0 100 0 0";
        assert_eq!(parse_process_cpu_ticks(stat), Some(42));
    }

    #[test]
    fn derives_cpu_and_queue_utilization() {
        let previous = CpuTimes {
            total: 100,
            idle: 40,
        };
        let current = CpuTimes {
            total: 200,
            idle: 70,
        };
        assert_eq!(cpu_usage_percent(previous, current), Some(70.0));
        assert_eq!(
            process_cpu_usage_percent(previous, current, 10, 25),
            Some(15.0)
        );
        let queue = SystemQueueLoad::new(3, 4);
        assert_eq!(queue.utilization_percent, Some(75.0));
    }
}
