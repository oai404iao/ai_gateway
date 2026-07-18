//! Background control-plane snapshot reloading and request-log persistence.

use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior, interval, timeout, timeout_at},
};

use crate::{
    admission::AdmissionRuntime,
    application::QueueRequestLogSink,
    application::{ControlPlaneCoordinator, ControlPlaneError},
    domain::RequestLogEvent,
    persistence::{
        ControlPlaneRepository, RepositoryError, RequestLogInsertOutcome, RequestLogRepository,
        RequestLogSettlementOutcome,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{ConfigError, RuntimeConfig, UpstreamConfig},
};

/// Bounds an individual database write so a stalled connection cannot stop the
/// single consumer from handling later events.
const REQUEST_LOG_INSERT_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds shutdown draining after the receiver closes and rejects new events.
const REQUEST_LOG_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
/// A durable reconciliation pass makes an inserted-but-unbilled log recover
/// after a worker restart or a transient settlement failure.
const SETTLEMENT_RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
const SETTLEMENT_RECOVERY_BATCH_SIZE: i64 = 100;

/// Owns the sole request-log consumer. Shutdown closes the receiver so no
/// sender clone can extend draining indefinitely.
pub struct RequestLogWorker {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl RequestLogWorker {
    #[must_use]
    pub fn start(repository: RequestLogRepository, capacity: usize) -> (QueueRequestLogSink, Self) {
        Self::start_inner(repository, capacity, None)
    }

    /// Starts persistence with immediate admission-state publication after a
    /// successful settlement. The durable database state remains authoritative;
    /// this only avoids waiting for the periodic control-plane reload before a
    /// same-process soft-quota precheck observes newly settled usage.
    #[must_use]
    pub fn start_with_admission(
        repository: RequestLogRepository,
        capacity: usize,
        admission: AdmissionRuntime,
    ) -> (QueueRequestLogSink, Self) {
        Self::start_inner(repository, capacity, Some(admission))
    }

    fn start_inner(
        repository: RequestLogRepository,
        capacity: usize,
        admission: Option<AdmissionRuntime>,
    ) -> (QueueRequestLogSink, Self) {
        let (sender, mut receiver) = mpsc::channel::<RequestLogEvent>(capacity);
        let (shutdown, mut shutdown_requested) = oneshot::channel();
        let sink = QueueRequestLogSink::new(sender);
        let task = tokio::spawn(async move {
            let mut recovery = interval(SETTLEMENT_RECOVERY_INTERVAL);
            recovery.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    event = receiver.recv() => match event {
                        Some(event) => persist_event(&repository, event, None, admission.as_ref()).await,
                        None => {
                            reconcile_settlements(&repository, None, admission.as_ref()).await;
                            return;
                        }
                    },
                    _ = recovery.tick() => reconcile_settlements(&repository, None, admission.as_ref()).await,
                    _ = &mut shutdown_requested => {
                        receiver.close();
                        let deadline = Instant::now() + REQUEST_LOG_DRAIN_TIMEOUT;
                        loop {
                            match timeout_at(deadline, receiver.recv()).await {
                                Ok(Some(event)) => persist_event(&repository, event, Some(deadline), admission.as_ref()).await,
                                Ok(None) => {
                                    reconcile_settlements(&repository, Some(deadline), admission.as_ref()).await;
                                    return;
                                }
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
    admission: Option<&AdmissionRuntime>,
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
        Ok(Ok(RequestLogInsertOutcome::Inserted)) => {
            settle_event(repository, id, drain_deadline, admission).await;
        }
        Ok(Ok(RequestLogInsertOutcome::ExactDuplicate)) => {
            tracing::debug!(request_log_id = %id, reason = "exact_duplicate", "request log already persisted");
            settle_event(repository, id, drain_deadline, admission).await;
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

async fn reconcile_settlements(
    repository: &RequestLogRepository,
    drain_deadline: Option<Instant>,
    admission: Option<&AdmissionRuntime>,
) {
    let duration = drain_deadline.map_or(REQUEST_LOG_INSERT_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .min(REQUEST_LOG_INSERT_TIMEOUT)
    });
    if duration.is_zero() {
        return;
    }
    match timeout(
        duration,
        repository.settle_pending(SETTLEMENT_RECOVERY_BATCH_SIZE),
    )
    .await
    {
        Ok(Ok(outcomes)) => {
            for outcome in outcomes {
                handle_settlement_outcome(None, outcome, admission);
            }
        }
        Ok(Err(_)) => {
            tracing::error!(
                reason = "settlement_recovery_failed",
                "request-log settlement recovery failed"
            );
        }
        Err(_) => {
            tracing::error!(
                reason = "settlement_recovery_timeout",
                "request-log settlement recovery timed out"
            );
        }
    }
}

async fn settle_event(
    repository: &RequestLogRepository,
    request_log_id: uuid::Uuid,
    drain_deadline: Option<Instant>,
    admission: Option<&AdmissionRuntime>,
) {
    let duration = drain_deadline.map_or(REQUEST_LOG_INSERT_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .min(REQUEST_LOG_INSERT_TIMEOUT)
    });
    if duration.is_zero() {
        tracing::warn!(request_log_id = %request_log_id, reason = "drain_timeout", "request-log settlement deferred after drain deadline");
        return;
    }
    match timeout(duration, repository.settle(request_log_id)).await {
        Ok(Ok(outcome)) => handle_settlement_outcome(Some(request_log_id), outcome, admission),
        Ok(Err(_)) => {
            tracing::error!(request_log_id = %request_log_id, reason = "settlement_failed", "request-log settlement failed; durable recovery will retry");
        }
        Err(_) => {
            tracing::error!(request_log_id = %request_log_id, reason = "settlement_timeout", "request-log settlement timed out; durable recovery will retry");
        }
    }
}

fn handle_settlement_outcome(
    request_log_id: Option<uuid::Uuid>,
    outcome: RequestLogSettlementOutcome,
    admission: Option<&AdmissionRuntime>,
) {
    match outcome {
        RequestLogSettlementOutcome::Settled {
            request_log_id,
            api_key_id,
            quota_used_amount,
        } => {
            if let Some(admission) = admission {
                admission.record_settled_quota_usage(api_key_id, quota_used_amount);
            }
            tracing::debug!(request_log_id = %request_log_id, "request log settled");
        }
        RequestLogSettlementOutcome::AlreadyBilled => {
            tracing::debug!(request_log_id = ?request_log_id, reason = "already_billed", "request-log settlement already applied");
        }
        RequestLogSettlementOutcome::NotBillable => {
            tracing::debug!(request_log_id = ?request_log_id, reason = "not_billable", "request log has no settled cost");
        }
        RequestLogSettlementOutcome::CurrencyMismatch => {
            tracing::error!(request_log_id = ?request_log_id, reason = "currency_mismatch", "request-log settlement requires matching user and request currencies");
        }
        RequestLogSettlementOutcome::AccountMismatch => {
            tracing::error!(request_log_id = ?request_log_id, reason = "account_mismatch", "request-log settlement found inconsistent API-key ownership");
        }
        RequestLogSettlementOutcome::NotFound => {
            tracing::debug!(request_log_id = ?request_log_id, reason = "not_found", "request log disappeared before settlement");
        }
    }
}

#[derive(Clone)]
pub struct ControlPlaneReloader {
    coordinator: ControlPlaneCoordinator,
}
impl ControlPlaneReloader {
    #[must_use]
    pub fn new(
        repository: ControlPlaneRepository,
        runtime: Arc<RuntimeConfig>,
        upstream_defaults: UpstreamConfig,
    ) -> Self {
        Self {
            coordinator: ControlPlaneCoordinator::new(
                repository,
                runtime,
                RoutingRuntime::new(PassiveHealthPolicy::default()),
                upstream_defaults,
            ),
        }
    }
    #[must_use]
    pub fn from_coordinator(coordinator: ControlPlaneCoordinator) -> Self {
        Self { coordinator }
    }
    #[must_use]
    pub fn with_routing(mut self, routing: RoutingRuntime) -> Self {
        self.coordinator = self.coordinator.with_routing(routing);
        self
    }
    pub async fn reload(&self) -> Result<(), ReloadError> {
        self.coordinator.reload().await.map_err(ReloadError::from)
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
    #[error("configured admin actor is not active")]
    InvalidActor,
}
impl From<ControlPlaneError> for ReloadError {
    fn from(value: ControlPlaneError) -> Self {
        match value {
            ControlPlaneError::Repository(error) => Self::Repository(error),
            ControlPlaneError::Compile(error) => Self::Compile(error),
            ControlPlaneError::InvalidActor => Self::InvalidActor,
        }
    }
}
