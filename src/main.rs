use std::{
    error::Error,
    io::{self, Read},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use ai_gateway::{
    admission::AdmissionRuntime,
    application::{
        AutomaticDisableWorker, ChannelModelDiscoveryService, CodexConnectorService,
        ConsoleAuthService, ControlPlaneCoordinator, ModelSyncService, ProxyService,
        ProxyTestService, RequestLogSink, SystemMetricsService, UpstreamConnectorRegistry,
        hash_console_password,
    },
    http,
    models_dev::ModelsDevClient,
    observability,
    persistence::{
        AuthRepository, ControlPlaneRepository, MIGRATOR, RequestLogRepository,
        SystemAutomaticDisableSettingsInput, SystemPassiveHealthSettingsInput,
        SystemRequestRetrySettingsInput, SystemScheduledTestingSettingsInput,
        SystemSessionAffinitySettingsInput, SystemSettingsInput, SystemUpstreamSettingsInput,
        SystemWebSocketSettingsInput,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{AppConfig, RuntimeConfig, compile_runtime_config},
    upstream::UpstreamClientRegistry,
    workers::{
        ChannelProbeWorker, CodexCredentialWorker, ControlPlaneReloader, DurableRequestLogWorker,
        SpendLeaderboardWorker,
    },
};
use axum::{Router, body::Body};
use chrono::Utc;
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::{conn::auto::Builder as AutoBuilder, graceful::GracefulConnection},
    service::TowerToHyperService,
};
use sqlx::postgres::PgPoolOptions;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{JoinError, JoinSet},
    time::timeout,
};
use tower::ServiceExt;

const DEFAULT_CONFIG_PATH: &str = "./config/config.toml";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match parse_command(std::env::args().skip(1).collect())? {
        Command::Serve { config_path } => serve(config_path).await,
        Command::Version => {
            println!("ai-gateway {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::BootstrapAdmin {
            config_path,
            email,
            display_name,
        } => bootstrap_admin(config_path, email, display_name).await,
        Command::ResetAdminPassword { config_path, email } => {
            reset_admin_password(config_path, email).await
        }
    }
}

async fn serve(config_path: PathBuf) -> Result<(), Box<dyn Error>> {
    let gateway_started_at = Utc::now();
    let gateway_started = Instant::now();
    let config = AppConfig::load(&config_path)?.validate()?;
    let _log_guard = observability::init(&config.observability.filter);
    let database_connect_options = config.database.connect_options()?;

    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(Duration::from_secs(config.database.connect_timeout_seconds))
        .connect_with(
            database_connect_options
                .clone()
                .application_name("ai-gateway-control-plane"),
        )
        .await?;
    MIGRATOR.run(&pool).await?;
    let repository = ControlPlaneRepository::new(pool.clone());
    repository
        .ensure_system_settings(SystemSettingsInput {
            api_hosts: Vec::new(),
            upstream: SystemUpstreamSettingsInput {
                connect_timeout_seconds: config.upstream.connect_timeout_seconds,
                response_header_timeout_seconds: config.upstream.response_header_timeout_seconds,
                images_response_header_timeout_seconds: config
                    .upstream
                    .images_response_header_timeout_seconds,
                standalone_web_search_response_header_timeout_seconds: config
                    .upstream
                    .standalone_web_search_response_header_timeout_seconds,
                stream_idle_timeout_seconds: config.upstream.stream_idle_timeout_seconds,
            },
            request_retry: SystemRequestRetrySettingsInput {
                enabled: config.request_retry.enabled,
                max_retries: config.request_retry.max_retries,
            },
            passive_health: SystemPassiveHealthSettingsInput {
                connection_failure_threshold: config.passive_health.connection_failure_threshold,
                cooldown_seconds: config.passive_health.cooldown_seconds,
            },
            automatic_disable: SystemAutomaticDisableSettingsInput {
                enabled: config.automatic_disable.enabled,
                error_status_codes: config.automatic_disable.error_status_codes,
                error_message_keywords: config.automatic_disable.error_message_keywords,
            },
            scheduled_testing: SystemScheduledTestingSettingsInput {
                mode: config.scheduled_testing.mode,
                auto_recover: config.scheduled_testing.auto_recover,
                interval_minutes: config.scheduled_testing.interval_minutes,
                prompt: config.scheduled_testing.prompt,
            },
            session_affinity: SystemSessionAffinitySettingsInput {
                enabled: config.session_affinity.enabled,
                max_entries: config.session_affinity.max_entries,
                default_ttl_seconds: config.session_affinity.default_ttl_seconds,
                rules: config.session_affinity.rules,
            },
            websocket: SystemWebSocketSettingsInput::default(),
            codex: Default::default(),
            mcp: config.mcp,
        })
        .await?;
    let system_probe_identity = repository.ensure_system_probe_identity().await?;
    let initial = compile_runtime_config(repository.load_runtime().await?)?;
    let initial_passive_health = initial.system_settings().passive_health();
    let runtime = Arc::new(RuntimeConfig::new(initial));

    let address = format!("{}:{}", config.server.host, config.server.port);
    let admission = AdmissionRuntime::new();
    let request_log_connect_options =
        database_connect_options.application_name("ai-gateway-request-log");
    let request_log_pool = PgPoolOptions::new()
        .max_connections(config.request_logging.database_max_connections)
        .acquire_timeout(Duration::from_secs(config.database.connect_timeout_seconds))
        .connect_with(request_log_connect_options)
        .await?;
    let spend_leaderboard_repository = RequestLogRepository::new(request_log_pool.clone());
    let (request_log_sink, request_log_worker) = DurableRequestLogWorker::start_with_admission(
        RequestLogRepository::new(request_log_pool),
        &config.request_logging,
        admission.clone(),
    )
    .await?;
    let request_log_monitor = request_log_worker.monitor();
    let request_log_sink: Arc<dyn RequestLogSink> = Arc::new(request_log_sink);
    let routing = RoutingRuntime::new(PassiveHealthPolicy {
        connection_failure_threshold: initial_passive_health.connection_failure_threshold(),
        cooldown: initial_passive_health.cooldown(),
    });
    routing.reconcile(&runtime.snapshot());
    let upstream_clients = Arc::new(UpstreamClientRegistry::new());
    let coordinator = ControlPlaneCoordinator::new_with_upstream_registry(
        repository.clone(),
        Arc::clone(&runtime),
        routing.clone(),
        Arc::clone(&upstream_clients),
    )?;
    let (automatic_disable_service, automatic_disable_worker) =
        AutomaticDisableWorker::start(coordinator.clone());
    let codex_connector = CodexConnectorService::new(
        repository.clone(),
        coordinator.clone(),
        Arc::clone(&runtime),
        Arc::clone(&upstream_clients),
    )
    .await?;
    let codex_credential_worker = CodexCredentialWorker::start(codex_connector.clone());
    let connectors = UpstreamConnectorRegistry::default().with_codex(codex_connector.clone());
    let proxy = ProxyService::with_dependencies_and_registry_and_automation(
        Arc::clone(&runtime),
        &config.request_limits,
        Arc::clone(&upstream_clients),
        Arc::clone(&request_log_sink),
        routing.clone(),
        admission.clone(),
        Some(automatic_disable_service.clone()),
    )?
    .with_connector_registry(connectors);
    let system_metrics = SystemMetricsService::new_at(
        pool.clone(),
        config.database.max_connections,
        gateway_started_at,
        gateway_started,
    )
    .with_runtime(
        admission,
        routing.clone(),
        request_log_monitor,
        automatic_disable_service.clone(),
    )
    .with_websocket_proxy(proxy.clone());
    let channel_probe_worker = ChannelProbeWorker::start(
        Arc::clone(&runtime),
        coordinator.clone(),
        Arc::clone(&upstream_clients),
        Arc::clone(&request_log_sink),
        automatic_disable_service,
        system_probe_identity,
    );
    let spend_leaderboard_worker = config
        .console
        .as_ref()
        .map(|_| SpendLeaderboardWorker::start(spend_leaderboard_repository));
    ControlPlaneReloader::from_coordinator(coordinator.clone()).spawn(Duration::from_secs(
        config.runtime_config.reload_interval_seconds,
    ));
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(%address, "AI gateway public listener enabled");
    #[cfg(feature = "mcp-server")]
    let mut public_router = http::router(proxy.clone());
    #[cfg(not(feature = "mcp-server"))]
    let public_router = http::router(proxy.clone());
    #[cfg(feature = "mcp-server")]
    let mcp_service = {
        let service = ai_gateway::mcp::McpService::new(proxy.clone(), Arc::clone(&runtime));
        public_router = public_router.merge(service.clone().router());
        let mcp = runtime.snapshot();
        let mcp = mcp.system_settings().mcp();
        tracing::info!(
            enabled = mcp.enabled(),
            public_base_url = mcp.public_base_url(),
            legacy_2025_11_25 = mcp.allow_legacy_2025_11_25(),
            "AI gateway MCP transport initialized"
        );
        Some(service)
    };

    let console = if let Some(console) = config.console.as_ref() {
        let auth =
            ConsoleAuthService::from_config(AuthRepository::new(pool.clone()), &console.auth)?;
        let channel_models =
            ChannelModelDiscoveryService::new(Arc::clone(&runtime), Arc::clone(&upstream_clients));
        let proxy_tests = ProxyTestService::new(
            repository.clone(),
            Arc::clone(&runtime),
            Arc::clone(&upstream_clients),
        );
        let model_sync = ModelSyncService::new(
            coordinator.clone(),
            ModelsDevClient::new(&config.models_sync)?,
            config.models_sync.max_selections,
        );
        let listener = TcpListener::bind(console.address).await?;
        tracing::info!(address = %console.address, "AI gateway Console listener enabled");
        let api_router = http::console::router(http::console::ConsoleState {
            coordinator: coordinator.clone(),
            codex_connector,
            channel_models,
            proxy_tests,
            model_sync,
            auth,
            request_logs: RequestLogRepository::new(pool.clone()),
            system_metrics,
            console_body_bytes: config.request_limits.console_body_bytes,
            auth_body_bytes: config.request_limits.auth_body_bytes,
            allowed_origins: console.allowed_origins.clone(),
        });
        // The embedded UI router is merged after the API router so explicit
        // `/console/v1/*` routes take precedence over the SPA fallback.
        #[cfg(feature = "embedded-console-ui")]
        let console_router = if console.ui_enabled {
            tracing::info!("Console embedded web UI enabled");
            api_router.merge(http::console_ui::router())
        } else {
            api_router
        };
        #[cfg(not(feature = "embedded-console-ui"))]
        let console_router = api_router;
        Some((listener, console_router))
    } else {
        None
    };

    let websocket_proxy = proxy.clone();
    let serve_result = run_servers(
        (listener, public_router),
        console,
        Duration::from_secs(config.server.shutdown_grace_period_seconds),
        websocket_proxy,
        #[cfg(feature = "mcp-server")]
        mcp_service,
    )
    .await;
    channel_probe_worker.shutdown().await;
    codex_credential_worker.shutdown().await;
    automatic_disable_worker.shutdown().await;
    if let Some(worker) = spend_leaderboard_worker {
        worker.shutdown().await;
    }
    request_log_worker.shutdown().await;
    serve_result?;
    Ok(())
}

async fn bootstrap_admin(
    config_path: PathBuf,
    email: String,
    display_name: String,
) -> Result<(), Box<dyn Error>> {
    let config = AppConfig::load(&config_path)?.validate()?;
    let _log_guard = observability::init(&config.observability.filter);
    let password = read_password_from_stdin()?;
    let password_hash = hash_console_password(password).await?;
    let database_connect_options = config.database.connect_options()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(Duration::from_secs(config.database.connect_timeout_seconds))
        .connect_with(database_connect_options.application_name("ai-gateway-bootstrap"))
        .await?;
    MIGRATOR.run(&pool).await?;
    let id = AuthRepository::new(pool)
        .bootstrap_admin(&email, &display_name, &password_hash)
        .await?;
    println!("bootstrap administrator created: {id}");
    Ok(())
}

async fn reset_admin_password(config_path: PathBuf, email: String) -> Result<(), Box<dyn Error>> {
    let config = AppConfig::load(&config_path)?.validate()?;
    let _log_guard = observability::init(&config.observability.filter);
    let password = read_password_from_stdin()?;
    let password_hash = hash_console_password(password).await?;
    let database_connect_options = config.database.connect_options()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(Duration::from_secs(config.database.connect_timeout_seconds))
        .connect_with(database_connect_options.application_name("ai-gateway-password-reset"))
        .await?;
    MIGRATOR.run(&pool).await?;
    let reset = AuthRepository::new(pool)
        .reset_active_admin_password(&email, &password_hash)
        .await?;
    if !reset {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no active administrator exists with the supplied email",
        )
        .into());
    }
    println!("administrator password reset: {email}");
    Ok(())
}

fn read_password_from_stdin() -> Result<String, io::Error> {
    let mut password = String::new();
    io::stdin().read_to_string(&mut password)?;
    while matches!(password.as_bytes().last(), Some(b'\n' | b'\r')) {
        password.pop();
    }
    Ok(password)
}

enum Command {
    Serve {
        config_path: PathBuf,
    },
    Version,
    BootstrapAdmin {
        config_path: PathBuf,
        email: String,
        display_name: String,
    },
    ResetAdminPassword {
        config_path: PathBuf,
        email: String,
    },
}

fn parse_command(arguments: Vec<String>) -> Result<Command, io::Error> {
    let Some(command) = arguments.first() else {
        return Ok(Command::Serve {
            config_path: PathBuf::from(DEFAULT_CONFIG_PATH),
        });
    };
    if matches!(command.as_str(), "--version" | "-V") {
        if arguments.len() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--version does not accept additional arguments",
            ));
        }
        return Ok(Command::Version);
    }
    if command == "bootstrap-admin" {
        let mut config_path = PathBuf::from(DEFAULT_CONFIG_PATH);
        let mut email = None;
        let mut display_name = None;
        let mut password_stdin = false;
        let mut index = 1;
        while index < arguments.len() {
            let flag = &arguments[index];
            index += 1;
            if flag == "--password-stdin" {
                password_stdin = true;
                continue;
            }
            let value = arguments.get(index).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "missing bootstrap-admin flag value",
                )
            })?;
            index += 1;
            match flag.as_str() {
                "--email" => email = Some(value.clone()),
                "--display-name" => display_name = Some(value.clone()),
                "--config" => config_path = PathBuf::from(value),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "unknown bootstrap-admin flag",
                    ));
                }
            }
        }
        if !password_stdin {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bootstrap-admin requires --password-stdin",
            ));
        }
        return Ok(Command::BootstrapAdmin {
            config_path,
            email: email.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "--email is required")
            })?,
            display_name: display_name.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "--display-name is required")
            })?,
        });
    }

    if command == "reset-admin-password" {
        let mut config_path = PathBuf::from(DEFAULT_CONFIG_PATH);
        let mut email = None;
        let mut password_stdin = false;
        let mut index = 1;
        while index < arguments.len() {
            let flag = &arguments[index];
            index += 1;
            if flag == "--password-stdin" {
                password_stdin = true;
                continue;
            }
            let value = arguments.get(index).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "missing reset-admin-password flag value",
                )
            })?;
            index += 1;
            match flag.as_str() {
                "--email" => email = Some(value.clone()),
                "--config" => config_path = PathBuf::from(value),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "unknown reset-admin-password flag",
                    ));
                }
            }
        }
        if !password_stdin {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reset-admin-password requires --password-stdin",
            ));
        }
        return Ok(Command::ResetAdminPassword {
            config_path,
            email: email.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "--email is required")
            })?,
        });
    }

    if arguments.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: ai-gateway [--version] | ai-gateway [./config/config.toml] | ai-gateway bootstrap-admin --email EMAIL --display-name NAME --password-stdin [--config ./config/config.toml] | ai-gateway reset-admin-password --email EMAIL --password-stdin [--config ./config/config.toml]",
        ));
    }
    Ok(Command::Serve {
        config_path: PathBuf::from(command),
    })
}

/// Runs public and Console listeners from one shutdown signal. Their routers
/// never share a route tree, only the shutdown lifecycle.
async fn run_servers(
    public: (TcpListener, Router),
    console: Option<(TcpListener, Router)>,
    shutdown_grace_period: Duration,
    websocket_proxy: ProxyService,
    #[cfg(feature = "mcp-server")] mcp_service: Option<ai_gateway::mcp::McpService>,
) -> Result<(), std::io::Error> {
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let mut servers = JoinSet::new();
    servers.spawn(run_server_until(
        public.0,
        public.1,
        shutdown_grace_period,
        shutdown_receiver.clone(),
    ));
    if let Some((listener, router)) = console {
        servers.spawn(run_server_until(
            listener,
            router,
            shutdown_grace_period,
            shutdown_receiver.clone(),
        ));
    }
    let first_error = tokio::select! {
        _ = shutdown_signal() => None,
        result = servers.join_next() => match result {
            Some(Ok(Err(error))) => Some(error),
            Some(Err(error)) => Some(std::io::Error::other(error)),
            _ => None,
        },
    };
    websocket_proxy.begin_websocket_shutdown();
    let _ = shutdown_sender.send(());
    let mut error = first_error;
    let deadline = tokio::time::Instant::now() + shutdown_grace_period;
    let mut force_close = false;
    loop {
        if servers.is_empty() && websocket_proxy.active_websocket_sessions() == 0 {
            break;
        }
        tokio::select! {
            result = servers.join_next(), if !servers.is_empty() => {
                let Some(result) = result else {
                    continue;
                };
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(next)) if error.is_none() => error = Some(next),
                    Ok(Err(next)) => tracing::warn!(%next, "listener task failed while draining"),
                    Err(next) => tracing::warn!(%next, "listener task join failed while draining"),
                }
            }
            _ = websocket_proxy.wait_for_websocket_shutdown(),
                if websocket_proxy.active_websocket_sessions() > 0 => {}
            _ = tokio::time::sleep_until(deadline) => {
                force_close = true;
                break;
            }
        }
    }
    if force_close {
        tracing::warn!(
            grace_period_seconds = shutdown_grace_period.as_secs(),
            "graceful shutdown deadline expired; force-closing HTTP and WebSocket connections"
        );
        #[cfg(feature = "mcp-server")]
        if let Some(service) = mcp_service.as_ref() {
            service.begin_shutdown();
        }
        websocket_proxy.force_websocket_shutdown();
        servers.abort_all();
        while let Some(result) = servers.join_next().await {
            if let Err(next) = result
                && !next.is_cancelled()
            {
                tracing::warn!(%next, "listener task join failed while force-closing");
            }
        }
        if timeout(
            Duration::from_secs(1),
            websocket_proxy.wait_for_websocket_shutdown(),
        )
        .await
        .is_err()
        {
            tracing::warn!("Responses WebSocket tasks did not stop after forced shutdown");
        }
    }
    #[cfg(feature = "mcp-server")]
    if let Some(service) = mcp_service.as_ref() {
        service.begin_shutdown();
    }
    error.map_or(Ok(()), Err)
}

async fn run_server_until(
    listener: TcpListener,
    router: Router,
    shutdown_grace_period: Duration,
    mut shutdown_receiver: watch::Receiver<()>,
) -> Result<(), std::io::Error> {
    let mut connections = JoinSet::new();

    let stop_reason = loop {
        tokio::select! {
            _ = shutdown_receiver.changed() => break StopReason::ShutdownSignal,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer_address)) => {
                    connections.spawn(serve_connection(
                        stream,
                        peer_address,
                        router.clone(),
                        shutdown_receiver.clone(),
                    ));
                }
                Err(error) => break StopReason::ListenerError(error),
            },
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(result) = result {
                    log_connection_task_result(result);
                }
            }
        }
    };

    match stop_reason {
        StopReason::ShutdownSignal => {
            drain_connections(&mut connections, shutdown_grace_period).await;
            Ok(())
        }
        StopReason::ListenerError(error) => {
            drain_connections(&mut connections, shutdown_grace_period).await;
            Err(error)
        }
    }
}

enum StopReason {
    ShutdownSignal,
    ListenerError(std::io::Error),
}

async fn serve_connection(
    stream: TcpStream,
    peer_address: SocketAddr,
    router: Router,
    mut shutdown: watch::Receiver<()>,
) {
    if let Err(error) = enable_tcp_nodelay(&stream) {
        tracing::warn!(%peer_address, %error, "failed to enable TCP_NODELAY");
    }
    let builder = AutoBuilder::new(TokioExecutor::new());
    let service = TowerToHyperService::new(
        router.map_request(|request: hyper::Request<Incoming>| request.map(Body::new)),
    );
    let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
    tokio::pin!(connection);

    tokio::select! {
        result = &mut connection => log_connection_result(peer_address, result),
        _ = shutdown.changed() => {
            GracefulConnection::graceful_shutdown(connection.as_mut());
            log_connection_result(peer_address, connection.await);
        }
    }
}

fn enable_tcp_nodelay(stream: &TcpStream) -> Result<(), std::io::Error> {
    stream.set_nodelay(true)
}

fn log_connection_result<E: std::fmt::Display>(peer_address: SocketAddr, result: Result<(), E>) {
    if let Err(error) = result {
        tracing::trace!(%peer_address, %error, "HTTP connection closed with error");
    }
}

async fn drain_connections(connections: &mut JoinSet<()>, shutdown_grace_period: Duration) {
    if connections.is_empty() {
        return;
    }

    if timeout(shutdown_grace_period, join_connections(connections))
        .await
        .is_err()
    {
        tracing::warn!(
            grace_period_seconds = shutdown_grace_period.as_secs(),
            "graceful shutdown deadline expired; force-closing remaining connections"
        );
        connections.abort_all();
        join_connections(connections).await;
    }
}

async fn join_connections(connections: &mut JoinSet<()>) {
    while let Some(result) = connections.join_next().await {
        log_connection_task_result(result);
    }
}

fn log_connection_task_result(result: Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_cancelled() {
            tracing::trace!("HTTP connection task cancelled");
        } else {
            tracing::warn!(%error, "HTTP connection task failed");
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("installing SIGTERM handler must succeed");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .expect("installing Ctrl-C handler must succeed");
    tracing::info!("shutdown signal received; draining in-flight requests");
}

#[cfg(test)]
mod tests {
    use std::{future::pending, net::Ipv4Addr, path::PathBuf, time::Duration};

    use tokio::{
        net::{TcpListener, TcpStream},
        sync::oneshot,
        task::JoinSet,
    };

    use super::{
        Command, DEFAULT_CONFIG_PATH, drain_connections, enable_tcp_nodelay, parse_command,
    };

    struct DropProbe(Option<oneshot::Sender<()>>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[test]
    fn default_config_is_the_current_directory_config_file() {
        let command = parse_command(Vec::new()).expect("default command must parse");
        let Command::Serve { config_path } = command else {
            panic!("expected serve command");
        };
        assert_eq!(DEFAULT_CONFIG_PATH, "./config/config.toml");
        assert_eq!(config_path, PathBuf::from("./config/config.toml"));
    }

    #[test]
    fn version_command_parses_without_configuration() {
        assert!(matches!(
            parse_command(vec!["--version".to_owned()]).unwrap(),
            Command::Version
        ));
        assert!(parse_command(vec!["--version".to_owned(), "extra".to_owned()]).is_err());
    }

    #[test]
    fn reset_admin_password_command_parses_its_required_arguments() {
        let command = parse_command(vec![
            "reset-admin-password".to_owned(),
            "--email".to_owned(),
            "admin@example.com".to_owned(),
            "--password-stdin".to_owned(),
            "--config".to_owned(),
            "./config/test.toml".to_owned(),
        ])
        .expect("reset command must parse");
        let Command::ResetAdminPassword { config_path, email } = command else {
            panic!("expected reset-admin-password command");
        };
        assert_eq!(email, "admin@example.com");
        assert_eq!(config_path, PathBuf::from("./config/test.toml"));
    }

    #[test]
    fn bootstrap_admin_rejects_the_removed_currency_option() {
        assert!(
            parse_command(vec![
                "bootstrap-admin".to_owned(),
                "--email".to_owned(),
                "admin@example.com".to_owned(),
                "--display-name".to_owned(),
                "Initial Admin".to_owned(),
                "--password-stdin".to_owned(),
                "--currency".to_owned(),
                "USD".to_owned(),
            ])
            .is_err()
        );
    }

    #[tokio::test]
    async fn accepted_connections_enable_tcp_nodelay() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = tokio::spawn(TcpStream::connect(address));
        let (server, _) = listener.accept().await.unwrap();
        let client = client.await.unwrap().unwrap();

        enable_tcp_nodelay(&server).unwrap();

        assert!(server.nodelay().unwrap());
        drop(client);
    }

    #[tokio::test]
    async fn grace_deadline_aborts_and_drops_tracked_connections() {
        let (started, started_receiver) = oneshot::channel();
        let (dropped, dropped_receiver) = oneshot::channel();
        let mut connections = JoinSet::new();
        connections.spawn(async move {
            let _probe = DropProbe(Some(dropped));
            let _ = started.send(());
            pending::<()>().await;
        });
        started_receiver.await.unwrap();

        drain_connections(&mut connections, Duration::ZERO).await;

        dropped_receiver.await.unwrap();
        assert!(connections.is_empty());
    }
}
