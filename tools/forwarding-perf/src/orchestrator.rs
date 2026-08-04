//! End-to-end lifecycle orchestration for isolated manual performance runs.

use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Command as StdCommand,
    time::Duration,
};

use chrono::Utc;
use serde::Serialize;
use tokio::{
    fs,
    net::TcpListener,
    process::Command,
    time::{Instant, sleep},
};
use uuid::Uuid;

use crate::{
    database::{TemporaryDatabase, default_database_admin_url},
    metrics::LoadResult,
    mock_upstream::MockStats,
    process::ManagedChild,
    report::{RunReport, ScenarioReport},
    scenario::{CLIENT_API_KEY, Profile, ProfileName, Scenario, profile},
};

const REQUEST_LOG_QUEUE_CAPACITY: usize = 1_024;

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub profile: ProfileName,
    pub database_admin_url: String,
    pub gateway_bin: PathBuf,
    pub report_root: PathBuf,
    pub keep_database: bool,
    repo_root: PathBuf,
}

impl RunOptions {
    pub fn defaults(repo_root: &Path) -> Self {
        Self {
            profile: ProfileName::Quick,
            database_admin_url: std::env::var("TEST_DATABASE_ADMIN_URL")
                .unwrap_or_else(|_| default_database_admin_url()),
            gateway_bin: repo_root.join("target/release/ai-gateway"),
            report_root: repo_root.join("target/perf/reports"),
            keep_database: false,
            repo_root: repo_root.to_path_buf(),
        }
    }
}

#[derive(Default)]
struct Resources {
    mock: Option<ManagedChild>,
    gateway: Option<ManagedChild>,
    database: Option<TemporaryDatabase>,
    runtime_config: Option<PathBuf>,
}

impl Resources {
    async fn stop_gateway(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Some(gateway) = self.gateway.take() {
            gateway.terminate(Duration::from_secs(90)).await?;
        }
        Ok(())
    }

    async fn cleanup(&mut self, keep_database: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut first_error: Option<Box<dyn Error + Send + Sync>> = None;
        if let Some(gateway) = self.gateway.take()
            && let Err(error) = gateway.terminate(Duration::from_secs(90)).await
        {
            first_error = Some(error);
        }
        if let Some(mock) = self.mock.take()
            && let Err(error) = mock.terminate(Duration::from_secs(5)).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Some(database) = self.database.take()
            && let Err(error) = database.cleanup(keep_database).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Some(path) = self.runtime_config.take() {
            let _ = fs::remove_file(path).await;
        }
        first_error.map_or(Ok(()), Err)
    }
}

pub async fn run(options: RunOptions) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut resources = Resources::default();
    let result = {
        let execution = execute(&options, &mut resources);
        tokio::pin!(execution);
        tokio::select! {
            result = &mut execution => result,
            signal = tokio::signal::ctrl_c() => {
                signal?;
                Err("performance run interrupted".into())
            }
        }
    };
    let cleanup = resources.cleanup(options.keep_database).await;
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn execute(
    options: &RunOptions,
    resources: &mut Resources,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !options.gateway_bin.is_file() {
        return Err(format!(
            "release gateway binary not found at {}; run the wrapper script or cargo build --release --package ai-gateway",
            options.gateway_bin.display()
        )
        .into());
    }
    let selected_profile = profile(options.profile);
    let run_id = format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let report_directory = options.report_root.join(&run_id);
    fs::create_dir_all(&report_directory).await?;

    let current_exe = std::env::current_exe()?;
    let mock_address = localhost(free_port().await?);
    let mock_url = format!("http://{mock_address}");
    let mut mock_command = Command::new(&current_exe);
    mock_command
        .arg("mock-upstream")
        .arg("--listen")
        .arg(mock_address.to_string());
    resources.mock = Some(ManagedChild::spawn(
        "mock upstream",
        &mut mock_command,
        &report_directory.join("mock.log"),
    )?);
    wait_for_ready(
        resources.mock.as_mut().expect("mock child stored"),
        &format!("{mock_url}/health"),
        reqwest::StatusCode::NO_CONTENT,
        None,
    )
    .await?;

    println!("creating isolated performance database");
    let database = TemporaryDatabase::create(
        &options.database_admin_url,
        &selected_profile.scenarios,
        &mock_url,
    )
    .await?;
    let database_name = database.name().to_owned();
    let database_url = database.database_url().to_owned();
    resources.database = Some(database);

    let gateway_address = localhost(free_port().await?);
    let gateway_url = format!("http://{gateway_address}");
    let runtime_config = report_directory.join("gateway.runtime.toml");
    write_gateway_config(
        &runtime_config,
        gateway_address,
        &database_url,
        &report_directory.join("request-log-spool"),
    )
    .await?;
    resources.runtime_config = Some(runtime_config.clone());
    let mut gateway_command = Command::new(&options.gateway_bin);
    gateway_command.arg(&runtime_config);
    resources.gateway = Some(ManagedChild::spawn(
        "ai-gateway",
        &mut gateway_command,
        &report_directory.join("gateway.log"),
    )?);
    wait_for_ready(
        resources.gateway.as_mut().expect("gateway child stored"),
        &format!("{gateway_url}/health"),
        reqwest::StatusCode::NO_CONTENT,
        None,
    )
    .await?;
    wait_for_ready(
        resources.gateway.as_mut().expect("gateway child stored"),
        &format!("{gateway_url}/v1/models"),
        reqwest::StatusCode::OK,
        Some(CLIENT_API_KEY),
    )
    .await?;

    let control_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut scenario_reports = Vec::with_capacity(selected_profile.scenarios.len());
    for scenario in &selected_profile.scenarios {
        println!(
            "running {}: direct baseline, then gateway (concurrency {})",
            scenario.name, scenario.concurrency
        );
        configure_mock(&control_client, &mock_url, scenario).await?;
        reset_mock(&control_client, &mock_url).await?;
        let direct = run_load_process(
            &current_exe,
            scenario,
            &mock_url,
            "direct",
            &report_directory,
        )
        .await?;
        let direct_mock = mock_stats(&control_client, &mock_url).await?;
        validate_mock_stats(&scenario.name, "direct", &direct_mock)?;

        reset_mock(&control_client, &mock_url).await?;
        let gateway = run_load_process(
            &current_exe,
            scenario,
            &gateway_url,
            "gateway",
            &report_directory,
        )
        .await?;
        let gateway_mock = mock_stats(&control_client, &mock_url).await?;
        validate_mock_stats(&scenario.name, "gateway", &gateway_mock)?;
        scenario_reports.push(ScenarioReport::new(
            scenario.clone(),
            direct,
            direct_mock,
            gateway,
            gateway_mock,
        ));
    }

    resources.stop_gateway().await?;
    let database = resources
        .database
        .as_ref()
        .expect("database remains available until report completion");
    for scenario_report in &mut scenario_reports {
        let persisted = database
            .request_log_stats(&scenario_report.scenario)
            .await?;
        scenario_report.attach_persisted_logs(persisted);
    }

    let report = RunReport {
        schema_version: 1,
        run_id: run_id.clone(),
        generated_at: Utc::now(),
        profile: selected_profile.name.to_string(),
        git_commit: command_output_in(&options.repo_root, "git", &["rev-parse", "HEAD"])
            .unwrap_or_else(|| "unknown".into()),
        git_dirty: command_output_in(&options.repo_root, "git", &["status", "--porcelain"])
            .is_some_and(|output| !output.is_empty()),
        rustc_version: command_output("rustc", &["--version"]).unwrap_or_else(|| "unknown".into()),
        operating_system: command_output("uname", &["-srmo"])
            .unwrap_or_else(|| format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
        logical_cpus: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        database_name,
        gateway_url,
        mock_url,
        request_log_queue_capacity: REQUEST_LOG_QUEUE_CAPACITY,
        scenarios: scenario_reports,
    };
    crate::report::write(&report_directory, &report).await?;
    write_scenario_document(&report_directory, &selected_profile).await?;
    if let Some(path) = resources.runtime_config.as_ref() {
        fs::remove_file(path).await?;
    }
    resources.runtime_config = None;

    println!(
        "performance report written to {}",
        report_directory.display()
    );
    if options.keep_database {
        println!(
            "temporary database {} was retained by request",
            report.database_name
        );
    }
    Ok(())
}

async fn run_load_process(
    executable: &Path,
    scenario: &Scenario,
    target: &str,
    label: &str,
    report_directory: &Path,
) -> Result<LoadResult, Box<dyn Error + Send + Sync>> {
    let result_path = report_directory.join(format!("{}-{label}.json", scenario.name));
    let log_path = report_directory.join(format!("{}-{label}.log", scenario.name));
    let mut command = Command::new(executable);
    command
        .arg("load-client")
        .arg("--scenario")
        .arg(&scenario.name)
        .arg("--target")
        .arg(target)
        .arg("--api-format")
        .arg(scenario.api_kind.short_name())
        .arg("--concurrency")
        .arg(scenario.concurrency.to_string())
        .arg("--warmup-seconds")
        .arg(scenario.warmup_seconds.to_string())
        .arg("--duration-seconds")
        .arg(scenario.duration_seconds.to_string())
        .arg("--timeout-seconds")
        .arg(scenario.timeout_seconds.to_string())
        .arg("--api-key")
        .arg(CLIENT_API_KEY)
        .arg("--model")
        .arg(&scenario.model)
        .arg("--output")
        .arg(&result_path);
    if scenario.streamed {
        command.arg("--stream");
    }
    let child = ManagedChild::spawn(
        format!("load client {} {label}", scenario.name),
        &mut command,
        &log_path,
    )?;
    let status = child.wait().await?;
    if !status.success() {
        return Err(format!(
            "load client for {} {label} exited with {status}; see {}",
            scenario.name,
            log_path.display()
        )
        .into());
    }
    let bytes = fs::read(&result_path).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn configure_mock(
    client: &reqwest::Client,
    mock_url: &str,
    scenario: &Scenario,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let response = client
        .post(format!("{mock_url}/__perf/config"))
        .json(&scenario.mock)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(format!(
            "mock configuration failed with status {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        )
        .into());
    }
    Ok(())
}

async fn reset_mock(
    client: &reqwest::Client,
    mock_url: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = client
            .post(format!("{mock_url}/__perf/reset"))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(());
        }
        if response.status() != reqwest::StatusCode::CONFLICT || Instant::now() >= deadline {
            return Err(format!(
                "mock counter reset failed with status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )
            .into());
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn mock_stats(
    client: &reqwest::Client,
    mock_url: &str,
) -> Result<MockStats, Box<dyn Error + Send + Sync>> {
    let response = client
        .get(format!("{mock_url}/__perf/stats"))
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

fn validate_mock_stats(
    scenario: &str,
    label: &str,
    stats: &MockStats,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if stats.current_in_flight != 0 {
        return Err(format!(
            "mock still has {} in-flight requests after {scenario} {label}",
            stats.current_in_flight
        )
        .into());
    }
    if stats.invalid_authorization != 0 {
        return Err(format!(
            "mock observed {} invalid Authorization headers during {scenario} {label}",
            stats.invalid_authorization
        )
        .into());
    }
    if stats
        .completed_requests
        .saturating_add(stats.cancelled_requests)
        != stats.accepted_requests
    {
        return Err(
            format!("mock request accounting was incomplete during {scenario} {label}").into(),
        );
    }
    Ok(())
}

async fn wait_for_ready(
    child: &mut ManagedChild,
    url: &str,
    expected_status: reqwest::StatusCode,
    api_key: Option<&str>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("child process exited before readiness with {status}").into());
        }
        let mut request = client.get(url);
        if let Some(api_key) = api_key {
            request = request.bearer_auth(api_key);
        }
        if request
            .send()
            .await
            .is_ok_and(|response| response.status() == expected_status)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {url} to become ready").into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn free_port() -> Result<u16, std::io::Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    Ok(listener.local_addr()?.port())
}

fn localhost(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

async fn write_gateway_config(
    path: &Path,
    address: SocketAddr,
    database_url: &str,
    spool_directory: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config = GatewayConfig {
        server: GatewayServer {
            host: address.ip().to_string(),
            port: address.port(),
            shutdown_grace_period_seconds: 5,
        },
        request_limits: GatewayRequestLimits {
            proxy_body_bytes: 1_048_576,
            console_body_bytes: 262_144,
            auth_body_bytes: 16_384,
        },
        database: GatewayDatabase {
            url: database_url.into(),
            max_connections: 10,
            connect_timeout_seconds: 5,
        },
        upstream: GatewayUpstream {
            connect_timeout_seconds: 5,
            response_header_timeout_seconds: 30,
            images_response_header_timeout_seconds: 300,
            stream_idle_timeout_seconds: 60,
        },
        runtime_config: GatewayReload {
            reload_interval_seconds: 3_600,
        },
        request_logging: GatewayRequestLogging {
            queue_capacity: REQUEST_LOG_QUEUE_CAPACITY,
            database_max_connections: 4,
            ingest_batch_size: 4_096,
            projection_batch_size: 2_048,
            settlement_batch_size: 4_096,
            settlement_interval_milliseconds: 500,
            spool_directory: spool_directory.to_path_buf(),
            spool_sync_interval_milliseconds: 10,
            spool_compaction_threshold_bytes: 256 * 1_024 * 1_024,
            metrics_interval_seconds: 3_600,
            shutdown_drain_seconds: 60,
        },
        passive_health: GatewayPassiveHealth {
            connection_failure_threshold: 3,
            cooldown_seconds: 30,
        },
        automatic_disable: GatewayAutomaticDisable {
            enabled: false,
            error_status_codes: Vec::new(),
            error_message_keywords: Vec::new(),
        },
        scheduled_testing: GatewayScheduledTesting {
            mode: "global".into(),
            auto_recover: true,
            interval_minutes: 60,
            prompt: "reply '1'".into(),
        },
        observability: GatewayObservability {
            filter: "ai_gateway=error,tower_http=error".into(),
        },
    };
    fs::write(path, toml::to_string_pretty(&config)?).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

async fn write_scenario_document(
    directory: &Path,
    profile: &Profile,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    #[derive(Serialize)]
    struct ScenarioDocument<'a> {
        profile: &'a str,
        scenarios: &'a [Scenario],
    }
    let document = ScenarioDocument {
        profile: profile.name.as_str(),
        scenarios: &profile.scenarios,
    };
    fs::write(
        directory.join("scenario.toml"),
        toml::to_string_pretty(&document)?,
    )
    .await?;
    Ok(())
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = StdCommand::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn command_output_in(directory: &Path, program: &str, arguments: &[&str]) -> Option<String> {
    let output = StdCommand::new(program)
        .current_dir(directory)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[derive(Serialize)]
struct GatewayConfig {
    server: GatewayServer,
    request_limits: GatewayRequestLimits,
    database: GatewayDatabase,
    upstream: GatewayUpstream,
    runtime_config: GatewayReload,
    request_logging: GatewayRequestLogging,
    passive_health: GatewayPassiveHealth,
    automatic_disable: GatewayAutomaticDisable,
    scheduled_testing: GatewayScheduledTesting,
    observability: GatewayObservability,
}

#[derive(Serialize)]
struct GatewayServer {
    host: String,
    port: u16,
    shutdown_grace_period_seconds: u64,
}

#[derive(Serialize)]
struct GatewayRequestLimits {
    proxy_body_bytes: usize,
    console_body_bytes: usize,
    auth_body_bytes: usize,
}

#[derive(Serialize)]
struct GatewayDatabase {
    url: String,
    max_connections: u32,
    connect_timeout_seconds: u64,
}

#[derive(Serialize)]
struct GatewayUpstream {
    connect_timeout_seconds: u64,
    response_header_timeout_seconds: u64,
    images_response_header_timeout_seconds: u64,
    stream_idle_timeout_seconds: u64,
}

#[derive(Serialize)]
struct GatewayReload {
    reload_interval_seconds: u64,
}

#[derive(Serialize)]
struct GatewayRequestLogging {
    queue_capacity: usize,
    database_max_connections: u32,
    ingest_batch_size: usize,
    projection_batch_size: usize,
    settlement_batch_size: i64,
    settlement_interval_milliseconds: u64,
    spool_directory: PathBuf,
    spool_sync_interval_milliseconds: u64,
    spool_compaction_threshold_bytes: u64,
    metrics_interval_seconds: u64,
    shutdown_drain_seconds: u64,
}

#[derive(Serialize)]
struct GatewayPassiveHealth {
    connection_failure_threshold: u32,
    cooldown_seconds: u64,
}

#[derive(Serialize)]
struct GatewayAutomaticDisable {
    enabled: bool,
    error_status_codes: Vec<u16>,
    error_message_keywords: Vec<String>,
}

#[derive(Serialize)]
struct GatewayScheduledTesting {
    mode: String,
    auto_recover: bool,
    interval_minutes: u64,
    prompt: String,
}

#[derive(Serialize)]
struct GatewayObservability {
    filter: String,
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use ai_gateway::runtime_config::AppConfig;
    use uuid::Uuid;

    use super::write_gateway_config;

    #[tokio::test]
    async fn generated_gateway_toml_matches_the_application_schema() {
        let path = std::env::temp_dir().join(format!(
            "ai-gateway-perf-config-{}.toml",
            Uuid::new_v4().simple()
        ));
        write_gateway_config(
            &path,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 30_000),
            "postgres://user:password@127.0.0.1:5432/ai_gateway_perf_test",
            &std::env::temp_dir()
                .join(format!("ai-gateway-perf-spool-{}", Uuid::new_v4().simple())),
        )
        .await
        .unwrap();
        AppConfig::load(&path).unwrap().validate().unwrap();
        tokio::fs::remove_file(path).await.unwrap();
    }
}
