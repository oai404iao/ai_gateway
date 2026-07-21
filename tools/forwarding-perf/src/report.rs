//! JSON and Markdown report models and rendering.

use std::{error::Error, path::Path};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::fs;

use crate::{
    database::PersistedLogStats, metrics::LoadResult, mock_upstream::MockStats, scenario::Scenario,
};

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub schema_version: u32,
    pub run_id: String,
    pub generated_at: DateTime<Utc>,
    pub profile: String,
    pub git_commit: String,
    pub git_dirty: bool,
    pub rustc_version: String,
    pub operating_system: String,
    pub logical_cpus: usize,
    pub database_name: String,
    pub gateway_url: String,
    pub mock_url: String,
    pub request_log_queue_capacity: usize,
    pub scenarios: Vec<ScenarioReport>,
}

#[derive(Debug, Serialize)]
pub struct ScenarioReport {
    pub scenario: Scenario,
    pub direct: LoadResult,
    pub direct_mock: MockStats,
    pub gateway: LoadResult,
    pub gateway_mock: MockStats,
    pub persisted_logs: PersistedLogStats,
    pub derived: DerivedMetrics,
}

impl ScenarioReport {
    pub fn new(
        scenario: Scenario,
        direct: LoadResult,
        direct_mock: MockStats,
        gateway: LoadResult,
        gateway_mock: MockStats,
    ) -> Self {
        Self {
            scenario,
            direct,
            direct_mock,
            gateway,
            gateway_mock,
            persisted_logs: PersistedLogStats::default(),
            derived: DerivedMetrics::default(),
        }
    }

    pub fn attach_persisted_logs(&mut self, persisted_logs: PersistedLogStats) {
        let expected_logs = self.gateway.all_requests();
        self.derived = DerivedMetrics {
            gateway_to_direct_success_rps_ratio: ratio(
                self.gateway.success_rps,
                self.direct.success_rps,
            ),
            gateway_p50_overhead_us: signed_difference(
                self.gateway.latency.p50_us,
                self.direct.latency.p50_us,
            ),
            gateway_p99_overhead_us: signed_difference(
                self.gateway.latency.p99_us,
                self.direct.latency.p99_us,
            ),
            expected_request_logs: expected_logs,
            request_log_persistence_ratio: ratio(persisted_logs.total as f64, expected_logs as f64),
        };
        self.persisted_logs = persisted_logs;
    }
}

#[derive(Debug, Default, Serialize)]
pub struct DerivedMetrics {
    pub gateway_to_direct_success_rps_ratio: f64,
    pub gateway_p50_overhead_us: i64,
    pub gateway_p99_overhead_us: i64,
    pub expected_request_logs: u64,
    pub request_log_persistence_ratio: f64,
}

pub async fn write(
    directory: &Path,
    report: &RunReport,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let json = serde_json::to_vec_pretty(report)?;
    fs::write(directory.join("report.json"), json).await?;
    fs::write(directory.join("report.md"), markdown(report)).await?;
    Ok(())
}

fn markdown(report: &RunReport) -> String {
    let mut output = String::new();
    output.push_str("# ai-gateway forwarding performance report\n\n");
    output.push_str(&format!("- Run ID: `{}`\n", report.run_id));
    output.push_str(&format!("- Generated: `{}`\n", report.generated_at));
    output.push_str(&format!("- Profile: `{}`\n", report.profile));
    output.push_str(&format!(
        "- Git: `{}`{}\n",
        report.git_commit,
        if report.git_dirty { " (dirty)" } else { "" }
    ));
    output.push_str(&format!("- Rust: `{}`\n", report.rustc_version));
    output.push_str(&format!("- OS: `{}`\n", report.operating_system));
    output.push_str(&format!("- Logical CPUs: `{}`\n", report.logical_cpus));
    output.push_str(&format!(
        "- Temporary database: `{}`\n",
        report.database_name
    ));
    output.push_str(&format!(
        "- Request-log wake queue capacity: `{}`\n\n",
        report.request_log_queue_capacity
    ));
    output.push_str(
        "Percentiles use a fixed-memory histogram with approximately 1.6% relative bucket precision above 127 microseconds.\n\n",
    );
    output.push_str("| Scenario | API | Stream | C | Direct RPS | Gateway RPS | Ratio | Direct p50 | Gateway p50 | Gateway p99 | Errors | Durable request logs |\n");
    output.push_str(
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for scenario in &report.scenarios {
        let errors = scenario
            .gateway
            .measurement
            .requests
            .saturating_sub(scenario.gateway.measurement.succeeded);
        output.push_str(&format!(
            "| {} | {} | {} | {} | {:.1} | {:.1} | {:.3} | {} | {} | {} | {} | {}/{} ({:.2}%) |\n",
            scenario.scenario.name,
            scenario.scenario.api_kind,
            scenario.scenario.streamed,
            scenario.scenario.concurrency,
            scenario.direct.success_rps,
            scenario.gateway.success_rps,
            scenario.derived.gateway_to_direct_success_rps_ratio,
            milliseconds(scenario.direct.latency.p50_us),
            milliseconds(scenario.gateway.latency.p50_us),
            milliseconds(scenario.gateway.latency.p99_us),
            errors,
            scenario.persisted_logs.total,
            scenario.derived.expected_request_logs,
            scenario.derived.request_log_persistence_ratio * 100.0,
        ));
    }

    output.push_str("\n## Scenario details\n");
    for scenario in &report.scenarios {
        output.push_str(&format!("\n### {}\n\n", scenario.scenario.name));
        output.push_str(&format!(
            "- Mock: `{:?}`, response delay `{}` ms, TTFT `{}` ms, chunk interval `{}` ms, chunks `{}`\n",
            scenario.scenario.mock.mode,
            scenario.scenario.mock.response_delay_ms,
            scenario.scenario.mock.ttft_ms,
            scenario.scenario.mock.chunk_interval_ms,
            scenario.scenario.mock.chunk_count,
        ));
        output.push_str(&format!(
            "- Direct latency: p50 `{}`, p95 `{}`, p99 `{}`, max `{}`\n",
            milliseconds(scenario.direct.latency.p50_us),
            milliseconds(scenario.direct.latency.p95_us),
            milliseconds(scenario.direct.latency.p99_us),
            milliseconds(scenario.direct.latency.max_us),
        ));
        output.push_str(&format!(
            "- Gateway latency: p50 `{}`, p95 `{}`, p99 `{}`, max `{}`\n",
            milliseconds(scenario.gateway.latency.p50_us),
            milliseconds(scenario.gateway.latency.p95_us),
            milliseconds(scenario.gateway.latency.p99_us),
            milliseconds(scenario.gateway.latency.max_us),
        ));
        if scenario.scenario.streamed {
            output.push_str(&format!(
                "- Gateway TTFT: p50 `{}`, p95 `{}`, p99 `{}`\n",
                milliseconds(scenario.gateway.ttft.p50_us),
                milliseconds(scenario.gateway.ttft.p95_us),
                milliseconds(scenario.gateway.ttft.p99_us),
            ));
        }
        output.push_str(&format!(
            "- Gateway overhead: p50 `{:+.3}` ms, p99 `{:+.3}` ms\n",
            scenario.derived.gateway_p50_overhead_us as f64 / 1_000.0,
            scenario.derived.gateway_p99_overhead_us as f64 / 1_000.0,
        ));
        output.push_str(&format!(
            "- Mock accepted/completed/cancelled: direct `{}/{}/{}`, gateway `{}/{}/{}`\n",
            scenario.direct_mock.accepted_requests,
            scenario.direct_mock.completed_requests,
            scenario.direct_mock.cancelled_requests,
            scenario.gateway_mock.accepted_requests,
            scenario.gateway_mock.completed_requests,
            scenario.gateway_mock.cancelled_requests,
        ));
        output.push_str(&format!(
            "- Durable outcomes: succeeded `{}`, failed `{}`, rejected `{}`, cancelled `{}`\n",
            scenario.persisted_logs.succeeded,
            scenario.persisted_logs.failed,
            scenario.persisted_logs.rejected,
            scenario.persisted_logs.cancelled,
        ));
    }
    output
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

fn signed_difference(left: u64, right: u64) -> i64 {
    i128::from(left)
        .saturating_sub(i128::from(right))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn milliseconds(microseconds: u64) -> String {
    format!("{:.3} ms", microseconds as f64 / 1_000.0)
}
