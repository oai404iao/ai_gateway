//! Background control-plane snapshot reloading and request-log persistence.

use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior, interval, timeout, timeout_at},
};

use crate::{
    application::QueueRequestLogSink,
    domain::RequestLogEvent,
    persistence::{
        ControlPlaneRepository, RepositoryError, RequestLogInsertOutcome, RequestLogRepository,
    },
    routing::RoutingRuntime,
    runtime_config::{ConfigError, RuntimeConfig, compile_control_plane},
};

/// Bounds an individual database write so a stalled connection cannot stop the
/// single consumer from handling later events.
const REQUEST_LOG_INSERT_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds shutdown draining after the receiver closes and rejects new events.
const REQUEST_LOG_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

/// Owns the sole request-log consumer. Shutdown closes the receiver so no
/// sender clone can extend draining indefinitely.
pub struct RequestLogWorker {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl RequestLogWorker {
    #[must_use]
    pub fn start(repository: RequestLogRepository, capacity: usize) -> (QueueRequestLogSink, Self) {
        let (sender, mut receiver) = mpsc::channel::<RequestLogEvent>(capacity);
        let (shutdown, mut shutdown_requested) = oneshot::channel();
        let sink = QueueRequestLogSink::new(sender);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = receiver.recv() => match event {
                        Some(event) => persist_event(&repository, event, None).await,
                        None => return,
                    },
                    _ = &mut shutdown_requested => {
                        receiver.close();
                        let deadline = Instant::now() + REQUEST_LOG_DRAIN_TIMEOUT;
                        loop {
                            match timeout_at(deadline, receiver.recv()).await {
                                Ok(Some(event)) => persist_event(&repository, event, Some(deadline)).await,
                                Ok(None) => return,
                                Err(_) => {
                                    tracing::warn!(reason = "drain_timeout", "request log worker stopped before its accepted queue fully drained");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });
        (sink, Self { shutdown, task })
    }

    pub async fn shutdown(self) {
        let Self { shutdown, mut task } = self;
        let _ = shutdown.send(());
        match timeout(
            REQUEST_LOG_DRAIN_TIMEOUT + REQUEST_LOG_INSERT_TIMEOUT,
            &mut task,
        )
        .await
        {
            Ok(Ok(())) => tracing::info!("request log worker drained and stopped"),
            Ok(Err(_)) => {
                tracing::error!(
                    reason = "worker_join_failed",
                    "request log worker terminated unexpectedly"
                )
            }
            Err(_) => {
                tracing::error!(
                    reason = "worker_join_timeout",
                    "request log worker did not stop after its drain deadline; aborting it"
                );
                task.abort();
                if task.await.is_err() {
                    tracing::warn!(
                        reason = "worker_aborted",
                        "request log worker aborted after shutdown timeout"
                    );
                }
            }
        }
    }
}

async fn persist_event(
    repository: &RequestLogRepository,
    event: RequestLogEvent,
    drain_deadline: Option<Instant>,
) {
    let id = event.id;
    let duration = drain_deadline.map_or(REQUEST_LOG_INSERT_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .min(REQUEST_LOG_INSERT_TIMEOUT)
    });
    if duration.is_zero() {
        tracing::warn!(request_log_id = %id, reason = "drain_timeout", "request log discarded after drain deadline");
        return;
    }
    match timeout(duration, repository.insert(&event)).await {
        Ok(Ok(RequestLogInsertOutcome::Inserted)) => {}
        Ok(Ok(RequestLogInsertOutcome::ExactDuplicate)) => {
            tracing::debug!(request_log_id = %id, reason = "exact_duplicate", "request log already persisted");
        }
        Ok(Err(RepositoryError::DuplicateConflict { .. })) => {
            tracing::error!(request_log_id = %id, reason = "duplicate_conflict", "request log id conflicts with immutable persisted facts");
        }
        Ok(Err(_)) => {
            tracing::error!(request_log_id = %id, reason = "insert_failed", "request log persistence failed");
        }
        Err(_) => {
            tracing::error!(request_log_id = %id, reason = "insert_timeout", "request log persistence timed out; continuing with later events");
        }
    }
}

#[derive(Clone)]
pub struct ControlPlaneReloader {
    repository: ControlPlaneRepository,
    runtime: Arc<RuntimeConfig>,
    serial: Arc<Mutex<()>>,
    routing: Option<RoutingRuntime>,
}
impl ControlPlaneReloader {
    #[must_use]
    pub fn new(repository: ControlPlaneRepository, runtime: Arc<RuntimeConfig>) -> Self {
        Self {
            repository,
            runtime,
            serial: Arc::new(Mutex::new(())),
            routing: None,
        }
    }
    #[must_use]
    pub fn with_routing(mut self, routing: RoutingRuntime) -> Self {
        self.routing = Some(routing);
        self
    }
    pub async fn reload(&self) -> Result<(), ReloadError> {
        let _guard = self.serial.lock().await;
        let records = self.repository.load().await?;
        let next = Arc::new(compile_control_plane(records)?);
        if let Some(routing) = &self.routing {
            routing.reconcile(&next);
        }
        self.runtime.replace_snapshot(Arc::clone(&next));
        Ok(())
    }
    pub fn spawn(self, frequency: Duration) {
        tokio::spawn(async move {
            let mut ticker = interval(frequency);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = self.reload().await {
                    tracing::error!(error = %error, "control-plane reload failed; retaining previous snapshot");
                }
            }
        });
    }
}
#[derive(Debug, Error)]
pub enum ReloadError {
    #[error("control-plane load failed")]
    Repository(#[from] RepositoryError),
    #[error("control-plane compilation failed: {0}")]
    Compile(#[from] ConfigError),
}
