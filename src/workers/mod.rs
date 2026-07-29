//! Background control-plane snapshot reloading and request-log persistence.

mod channel_probe;
mod codex;
mod durable_request_log;
mod spend_leaderboard;

use std::{future::pending, sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::{Instant, MissedTickBehavior, interval, sleep_until, timeout},
};

use crate::{
    admission::AdmissionRuntime,
    application::QueueRequestLogSink,
    application::{ControlPlaneCoordinator, ControlPlaneError},
    domain::RequestLogEvent,
    persistence::{
        ControlPlaneRepository, RepositoryError, RequestLogBatchInsertOutcome,
        RequestLogInsertOutcome, RequestLogRepository, RequestLogSettlementOutcome,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{ConfigError, RuntimeConfig},
};

pub use channel_probe::ChannelProbeWorker;
pub use codex::CodexCredentialWorker;
pub use durable_request_log::{DurableRequestLogWorker, DurableRequestLogWorkerStartError};
pub use spend_leaderboard::SpendLeaderboardWorker;

/// Bounds one batch database operation so a stalled connection cannot retain
/// an insert or settlement task indefinitely.
const REQUEST_LOG_INSERT_TIMEOUT: Duration = Duration::from_secs(5);
/// Keeps one multi-row insert below PostgreSQL's bind-parameter ceiling while
/// matching the default in-memory queue capacity.
const REQUEST_LOG_BATCH_SIZE: usize = 1_024;
/// Parallel multi-row inserts keep the bounded ingress queue draining while
/// PostgreSQL processes other batches and settlement on separate connections.
const REQUEST_LOG_INSERT_CONCURRENCY: usize = 2;
/// Bounds shutdown draining after the receiver closes and rejects new events.
const REQUEST_LOG_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
/// A durable reconciliation pass makes an inserted-but-unbilled log recover
/// after a worker restart or a transient settlement failure.
const SETTLEMENT_RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
const SETTLEMENT_RECOVERY_BATCH_SIZE: i64 = 1_024;
/// Durable rows are the source of truth, so a full notification queue may
/// drop only the in-memory settlement hint. Periodic recovery still finds it.
const SETTLEMENT_NOTIFICATION_CAPACITY: usize = 64;

/// Owns separate insert and settlement stages. Shutdown closes insertion
/// acceptance, drains accepted events, then lets settlement consume all hints
/// and reconcile any durable rows whose hint was dropped.
pub struct RequestLogWorker {
    shutdown: oneshot::Sender<()>,
    insert_task: JoinHandle<()>,
    settlement_task: JoinHandle<()>,
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
        let (sender, receiver) = mpsc::channel::<RequestLogEvent>(capacity);
        let (shutdown, shutdown_requested) = oneshot::channel();
        let (settlement_sender, settlement_receiver) =
            mpsc::channel::<Vec<uuid::Uuid>>(SETTLEMENT_NOTIFICATION_CAPACITY);
        let sink = QueueRequestLogSink::new(sender);
        let settlement_task = tokio::spawn(run_settlement_worker(
            repository.clone(),
            settlement_receiver,
            admission,
        ));
        let insert_task = tokio::spawn(run_insert_worker(
            repository,
            receiver,
            settlement_sender,
            shutdown_requested,
        ));
        (
            sink,
            Self {
                shutdown,
                insert_task,
                settlement_task,
            },
        )
    }

    pub async fn shutdown(self) {
        let Self {
            shutdown,
            mut insert_task,
            mut settlement_task,
        } = self;
        let _ = shutdown.send(());
        match timeout(
            REQUEST_LOG_DRAIN_TIMEOUT + REQUEST_LOG_INSERT_TIMEOUT,
            async {
                let insert = (&mut insert_task).await;
                let settlement = (&mut settlement_task).await;
                (insert, settlement)
            },
        )
        .await
        {
            Ok((Ok(()), Ok(()))) => tracing::info!("request log worker drained and stopped"),
            Ok((insert, settlement)) => {
                tracing::error!(
                    insert_join_failed = insert.is_err(),
                    settlement_join_failed = settlement.is_err(),
                    reason = "worker_join_failed",
                    "request log workers terminated unexpectedly"
                )
            }
            Err(_) => {
                tracing::error!(
                    reason = "worker_join_timeout",
                    "request log worker did not stop after its drain deadline; aborting it"
                );
                insert_task.abort();
                settlement_task.abort();
                let insert_aborted = insert_task.await.is_err();
                let settlement_aborted = settlement_task.await.is_err();
                if insert_aborted || settlement_aborted {
                    tracing::warn!(
                        reason = "worker_aborted",
                        "request log workers aborted after shutdown timeout"
                    );
                }
            }
        }
    }
}

async fn run_insert_worker(
    repository: RequestLogRepository,
    mut receiver: mpsc::Receiver<RequestLogEvent>,
    settlement_sender: mpsc::Sender<Vec<uuid::Uuid>>,
    mut shutdown_requested: oneshot::Receiver<()>,
) {
    let mut batches = JoinSet::new();
    let mut batch = Vec::with_capacity(REQUEST_LOG_BATCH_SIZE);
    let mut input_closed = false;
    let mut drain_deadline = None;
    loop {
        if input_closed && batches.is_empty() {
            return;
        }
        batch.clear();
        let can_receive = !input_closed && batches.len() < REQUEST_LOG_INSERT_CONCURRENCY;
        tokio::select! {
            _ = &mut shutdown_requested, if drain_deadline.is_none() => {
                receiver.close();
                drain_deadline = Some(Instant::now() + REQUEST_LOG_DRAIN_TIMEOUT);
            }
            received = receiver.recv_many(&mut batch, REQUEST_LOG_BATCH_SIZE), if can_receive => {
                if received == 0 {
                    input_closed = true;
                } else {
                    let events = std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(REQUEST_LOG_BATCH_SIZE),
                    );
                    let repository = repository.clone();
                    let settlement_sender = settlement_sender.clone();
                    let deadline = drain_deadline;
                    batches.spawn(async move {
                        persist_batch(
                            &repository,
                            &events,
                            deadline,
                            &settlement_sender,
                        )
                        .await;
                    });
                }
            }
            result = batches.join_next(), if !batches.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::error!(%error, reason = "insert_task_join_failed", "request-log insert batch task failed");
                }
            }
            _ = async {
                match drain_deadline {
                    Some(deadline) => sleep_until(deadline).await,
                    None => pending::<()>().await,
                }
            } => {
                tracing::warn!(
                    queued_events = receiver.len(),
                    in_flight_batches = batches.len(),
                    reason = "drain_timeout",
                    "request log worker stopped before its accepted queue fully drained"
                );
                batches.abort_all();
                while batches.join_next().await.is_some() {}
                return;
            }
        }
    }
}

async fn persist_batch(
    repository: &RequestLogRepository,
    events: &[RequestLogEvent],
    drain_deadline: Option<Instant>,
    settlement_sender: &mpsc::Sender<Vec<uuid::Uuid>>,
) {
    let duration = write_duration(drain_deadline);
    if duration.is_zero() {
        tracing::warn!(
            event_count = events.len(),
            reason = "drain_timeout",
            "request-log batch discarded after drain deadline"
        );
        return;
    }
    match timeout(duration, repository.insert_batch(events)).await {
        Ok(Ok(results)) => {
            let mut settlement_ids = Vec::with_capacity(results.len());
            for result in results {
                match result.outcome {
                    RequestLogBatchInsertOutcome::Inserted => {
                        settlement_ids.push(result.request_log_id);
                    }
                    RequestLogBatchInsertOutcome::ExactDuplicate => {
                        tracing::debug!(
                            request_log_id = %result.request_log_id,
                            reason = "exact_duplicate",
                            "request log already persisted"
                        );
                        settlement_ids.push(result.request_log_id);
                    }
                    RequestLogBatchInsertOutcome::DuplicateConflict => {
                        tracing::error!(
                            request_log_id = %result.request_log_id,
                            reason = "duplicate_conflict",
                            "request log id conflicts with immutable persisted facts"
                        );
                    }
                    RequestLogBatchInsertOutcome::InvalidResponseStatus { status } => {
                        tracing::error!(
                            request_log_id = %result.request_log_id,
                            status,
                            reason = "invalid_response_status",
                            "request log has an invalid response status"
                        );
                    }
                }
            }
            queue_settlement(settlement_sender, settlement_ids);
        }
        Ok(Err(_)) => {
            tracing::error!(
                event_count = events.len(),
                reason = "batch_insert_failed",
                "request-log batch insert failed; retrying events individually"
            );
            for event in events {
                persist_event(repository, event.clone(), drain_deadline, settlement_sender).await;
            }
        }
        Err(_) => {
            tracing::error!(
                event_count = events.len(),
                reason = "batch_insert_timeout",
                "request-log batch insert timed out"
            );
        }
    }
}

async fn persist_event(
    repository: &RequestLogRepository,
    event: RequestLogEvent,
    drain_deadline: Option<Instant>,
    settlement_sender: &mpsc::Sender<Vec<uuid::Uuid>>,
) {
    let id = event.id;
    let duration = write_duration(drain_deadline);
    if duration.is_zero() {
        tracing::warn!(request_log_id = %id, reason = "drain_timeout", "request log discarded after drain deadline");
        return;
    }
    match timeout(duration, repository.insert(&event)).await {
        Ok(Ok(RequestLogInsertOutcome::Inserted)) => {
            queue_settlement(settlement_sender, vec![id]);
        }
        Ok(Ok(RequestLogInsertOutcome::ExactDuplicate)) => {
            tracing::debug!(request_log_id = %id, reason = "exact_duplicate", "request log already persisted");
            queue_settlement(settlement_sender, vec![id]);
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

fn queue_settlement(
    settlement_sender: &mpsc::Sender<Vec<uuid::Uuid>>,
    request_log_ids: Vec<uuid::Uuid>,
) {
    if request_log_ids.is_empty() {
        return;
    }
    let event_count = request_log_ids.len();
    match settlement_sender.try_send(request_log_ids) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(
                event_count,
                reason = "settlement_notification_queue_full",
                "request-log settlement hint dropped; durable recovery will retry"
            );
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::error!(
                event_count,
                reason = "settlement_notification_queue_closed",
                "request-log settlement hint dropped; durable recovery will retry"
            );
        }
    }
}

async fn run_settlement_worker(
    repository: RequestLogRepository,
    mut receiver: mpsc::Receiver<Vec<uuid::Uuid>>,
    admission: Option<AdmissionRuntime>,
) {
    let mut recovery = interval(SETTLEMENT_RECOVERY_INTERVAL);
    recovery.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            request_log_ids = receiver.recv() => match request_log_ids {
                Some(request_log_ids) => {
                    settle_batch(&repository, &request_log_ids, None, admission.as_ref()).await;
                }
                None => {
                    reconcile_all_settlements(&repository, admission.as_ref()).await;
                    return;
                }
            },
            _ = recovery.tick() => {
                reconcile_settlements(&repository, None, admission.as_ref()).await;
            }
        }
    }
}

async fn reconcile_settlements(
    repository: &RequestLogRepository,
    drain_deadline: Option<Instant>,
    admission: Option<&AdmissionRuntime>,
) {
    let duration = write_duration(drain_deadline);
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

async fn settle_batch(
    repository: &RequestLogRepository,
    request_log_ids: &[uuid::Uuid],
    drain_deadline: Option<Instant>,
    admission: Option<&AdmissionRuntime>,
) {
    if request_log_ids.is_empty() {
        return;
    }
    let duration = write_duration(drain_deadline);
    if duration.is_zero() {
        tracing::warn!(
            event_count = request_log_ids.len(),
            reason = "drain_timeout",
            "request-log batch settlement deferred after drain deadline"
        );
        return;
    }
    match timeout(duration, repository.settle_batch(request_log_ids)).await {
        Ok(Ok(outcomes)) => {
            for (request_log_id, outcome) in outcomes {
                handle_settlement_outcome(Some(request_log_id), outcome, admission);
            }
        }
        Ok(Err(_)) => {
            tracing::error!(
                event_count = request_log_ids.len(),
                reason = "batch_settlement_failed",
                "request-log batch settlement failed; durable recovery will retry"
            );
        }
        Err(_) => {
            tracing::error!(
                event_count = request_log_ids.len(),
                reason = "batch_settlement_timeout",
                "request-log batch settlement timed out; durable recovery will retry"
            );
        }
    }
}

async fn reconcile_all_settlements(
    repository: &RequestLogRepository,
    admission: Option<&AdmissionRuntime>,
) {
    let deadline = Instant::now() + REQUEST_LOG_DRAIN_TIMEOUT;
    loop {
        let duration = write_duration(Some(deadline));
        if duration.is_zero() {
            tracing::warn!(
                reason = "drain_timeout",
                "request-log settlement recovery stopped at the drain deadline"
            );
            return;
        }
        match timeout(
            duration,
            repository.settle_pending(SETTLEMENT_RECOVERY_BATCH_SIZE),
        )
        .await
        {
            Ok(Ok(outcomes)) => {
                let count = outcomes.len();
                for outcome in outcomes {
                    handle_settlement_outcome(None, outcome, admission);
                }
                if count < SETTLEMENT_RECOVERY_BATCH_SIZE as usize {
                    return;
                }
            }
            Ok(Err(_)) => {
                tracing::error!(
                    reason = "settlement_recovery_failed",
                    "request-log settlement recovery failed during shutdown"
                );
                return;
            }
            Err(_) => {
                tracing::error!(
                    reason = "settlement_recovery_timeout",
                    "request-log settlement recovery timed out during shutdown"
                );
                return;
            }
        }
    }
}

fn write_duration(drain_deadline: Option<Instant>) -> Duration {
    drain_deadline.map_or(REQUEST_LOG_INSERT_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .min(REQUEST_LOG_INSERT_TIMEOUT)
    })
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
    pub fn new(repository: ControlPlaneRepository, runtime: Arc<RuntimeConfig>) -> Self {
        Self {
            coordinator: ControlPlaneCoordinator::new(
                repository,
                runtime,
                RoutingRuntime::new(PassiveHealthPolicy::default()),
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
