use std::{error::Error, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use ai_gateway::{
    application::{ControlPlaneCoordinator, ProxyService},
    http, observability,
    persistence::{ControlPlaneRepository, MIGRATOR, RequestLogRepository},
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{AppConfig, RuntimeConfig, compile_control_plane},
    workers::{ControlPlaneReloader, RequestLogWorker},
};
use axum::{Router, body::Body};
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    let config = AppConfig::load(&config_path)?.validate()?;

    let _log_guard = observability::init(&config.observability.filter);

    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(std::time::Duration::from_secs(
            config.database.connect_timeout_seconds,
        ))
        .connect(&config.database.url)
        .await?;
    MIGRATOR.run(&pool).await?;
    let repository = ControlPlaneRepository::new(pool.clone());
    let initial = compile_control_plane(repository.load().await?)?;
    let runtime = Arc::new(RuntimeConfig::new(initial));

    let address = format!("{}:{}", config.server.host, config.server.port);
    let (request_log_sink, request_log_worker) = RequestLogWorker::start(
        RequestLogRepository::new(pool.clone()),
        config.request_logging.queue_capacity,
    );
    let routing = RoutingRuntime::new(PassiveHealthPolicy {
        connection_failure_threshold: config.passive_health.connection_failure_threshold,
        cooldown: Duration::from_secs(config.passive_health.cooldown_seconds),
    });
    routing.reconcile(&runtime.snapshot());
    let proxy = ProxyService::with_log_sink_and_routing(
        Arc::clone(&runtime),
        config.server.max_request_body_bytes,
        &config.upstream,
        Arc::new(request_log_sink),
        routing.clone(),
    )?;
    let coordinator =
        ControlPlaneCoordinator::new(repository, Arc::clone(&runtime), routing.clone());
    ControlPlaneReloader::from_coordinator(coordinator.clone()).spawn(
        std::time::Duration::from_secs(config.runtime_config.reload_interval_seconds),
    );
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(%address, "AI gateway listening");
    let admin = if let Some(admin) = config.admin {
        coordinator.verify_active_actor(admin.actor_user_id).await?;
        let listener = TcpListener::bind(admin.address).await?;
        tracing::info!(address = %admin.address, "AI gateway admin listener enabled");
        Some((
            listener,
            http::admin::router(http::admin::AdminState {
                coordinator: coordinator.clone(),
                actor_user_id: admin.actor_user_id,
                verifier: admin.verifier,
            }),
        ))
    } else {
        None
    };

    let serve_result = run_servers(
        (listener, http::router(proxy)),
        admin,
        Duration::from_secs(config.server.shutdown_grace_period_seconds),
    )
    .await;
    request_log_worker.shutdown().await;
    serve_result?;
    Ok(())
}

/// Runs both listeners from one shutdown signal. The public and management
/// routers never share a route tree, only the shutdown lifecycle.
async fn run_servers(
    public: (TcpListener, Router),
    admin: Option<(TcpListener, Router)>,
    shutdown_grace_period: Duration,
) -> Result<(), std::io::Error> {
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let mut servers = JoinSet::new();
    servers.spawn(run_server_until(
        public.0,
        public.1,
        shutdown_grace_period,
        shutdown_receiver.clone(),
    ));
    if let Some((listener, router)) = admin {
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
    let _ = shutdown_sender.send(());
    let mut error = first_error;
    while let Some(result) = servers.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(next)) if error.is_none() => error = Some(next),
            Ok(Err(next)) => tracing::warn!(%next, "listener task failed while draining"),
            Err(next) => tracing::warn!(%next, "listener task join failed while draining"),
        }
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
    use std::{future::pending, time::Duration};

    use tokio::{sync::oneshot, task::JoinSet};

    use super::drain_connections;

    struct DropProbe(Option<oneshot::Sender<()>>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
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
