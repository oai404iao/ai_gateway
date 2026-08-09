//! Durable request-log ingestion, projection, settlement, and telemetry.

use std::{path::PathBuf, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    task::{JoinError, JoinHandle},
    time::{Instant, MissedTickBehavior, interval, sleep, timeout},
};

use crate::{
    admission::AdmissionRuntime,
    application::{DurableRequestLogSink, RequestLogPipelineMonitor},
    observability::{RequestLogPipelineMetrics, RequestLogPipelineMetricsSnapshot},
    persistence::{
        RequestLogBatchInsertOutcome, RequestLogIngestRecord, RequestLogRepository,
        RequestLogSettlementOutcome,
    },
    request_log_spool::{RequestLogSpool, SpoolReader},
    runtime_config::RequestLoggingConfig,
};

const DATABASE_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const INGEST_RETRY_DELAY: Duration = Duration::from_millis(50);
const PROJECTION_POLL_INTERVAL: Duration = Duration::from_secs(1);
const FIRST_PROJECTION_RETRY_SECONDS: i64 = 1;
const ISOLATED_PROJECTION_RETRY_SECONDS: i64 = 60;
const TELEMETRY_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const BACKLOG_STALE_AFTER: Duration = Duration::from_secs(30);
const DATABASE_POOL_SATURATED_AFTER: Duration = Duration::from_secs(30);

pub struct DurableRequestLogWorker {
    spool_shutdown: oneshot::Sender<()>,
    spool_task: JoinHandle<()>,
    spool_sync_shutdown: oneshot::Sender<()>,
    spool_sync_task: JoinHandle<()>,
    projection_task: JoinHandle<()>,
    settlement_shutdown: oneshot::Sender<()>,
    settlement_task: JoinHandle<()>,
    telemetry_shutdown: oneshot::Sender<()>,
    telemetry_task: JoinHandle<()>,
    spool: Arc<RequestLogSpool>,
    monitor: RequestLogPipelineMonitor,
    shutdown_drain: Duration,
}

impl DurableRequestLogWorker {
    pub async fn start(
        repository: RequestLogRepository,
        config: &RequestLoggingConfig,
    ) -> Result<(DurableRequestLogSink, Self), DurableRequestLogWorkerStartError> {
        Self::start_inner(repository, config, None).await
    }

    pub async fn start_with_admission(
        repository: RequestLogRepository,
        config: &RequestLoggingConfig,
        admission: AdmissionRuntime,
    ) -> Result<(DurableRequestLogSink, Self), DurableRequestLogWorkerStartError> {
        Self::start_inner(repository, config, Some(admission)).await
    }

    async fn start_inner(
        repository: RequestLogRepository,
        config: &RequestLoggingConfig,
        admission: Option<AdmissionRuntime>,
    ) -> Result<(DurableRequestLogSink, Self), DurableRequestLogWorkerStartError> {
        let settings = DurableRequestLogSettings::from(config);
        let directory = settings.spool_directory.clone();
        let compaction_threshold = settings.spool_compaction_threshold_bytes;
        let spool = Arc::new(
            tokio::task::spawn_blocking(move || {
                RequestLogSpool::open(directory, compaction_threshold)
            })
            .await?
            .map_err(|error| DurableRequestLogWorkerStartError::Spool {
                message: error.to_string(),
            })?,
        );
        let reader =
            spool
                .reader()
                .await
                .map_err(|error| DurableRequestLogWorkerStartError::Spool {
                    message: error.to_string(),
                })?;
        let metrics = Arc::new(RequestLogPipelineMetrics::default());
        let (wake_sender, wake_receiver) = mpsc::channel(config.queue_capacity);
        let (stage_sender, stage_receiver) = mpsc::channel(1);
        let (spool_shutdown, spool_shutdown_requested) = oneshot::channel();
        let (spool_sync_shutdown, spool_sync_shutdown_requested) = oneshot::channel();
        let (settlement_shutdown, settlement_shutdown_requested) = oneshot::channel();
        let (telemetry_shutdown, telemetry_shutdown_requested) = oneshot::channel();
        let monitor = RequestLogPipelineMonitor::new(
            repository.clone(),
            Arc::clone(&spool),
            wake_sender.clone(),
            config.queue_capacity,
            stage_sender.clone(),
            config.database_max_connections,
            Arc::clone(&metrics),
        );

        let spool_task = tokio::spawn(run_spool_ingest_worker(
            SpoolIngestContext {
                repository: repository.clone(),
                spool: Arc::clone(&spool),
                stage_sender,
                settings: settings.clone(),
                metrics: Arc::clone(&metrics),
            },
            reader,
            wake_receiver,
            spool_shutdown_requested,
        ));
        let projection_task = tokio::spawn(run_projection_worker(
            repository.clone(),
            stage_receiver,
            settings.clone(),
            Arc::clone(&metrics),
        ));
        let spool_sync_task = tokio::spawn(run_spool_sync_worker(
            Arc::clone(&spool),
            settings.spool_sync_interval,
            spool_sync_shutdown_requested,
        ));
        let settlement_task = tokio::spawn(run_durable_settlement_worker(
            repository.clone(),
            settlement_shutdown_requested,
            settings.clone(),
            admission,
            Arc::clone(&metrics),
        ));
        let telemetry_task = tokio::spawn(run_telemetry_reporter(
            repository,
            Arc::clone(&spool),
            telemetry_shutdown_requested,
            settings.clone(),
            Arc::clone(&metrics),
        ));
        let sink = DurableRequestLogSink::new(Arc::clone(&spool), wake_sender, metrics);
        tracing::info!(
            spool_directory = %spool.directory().display(),
            database_max_connections = config.database_max_connections,
            ingest_batch_size = config.ingest_batch_size,
            projection_batch_size = config.projection_batch_size,
            metrics_interval_seconds = config.metrics_interval_seconds,
            "durable request-log pipeline started"
        );
        Ok((
            sink,
            Self {
                spool_shutdown,
                spool_task,
                spool_sync_shutdown,
                spool_sync_task,
                projection_task,
                settlement_shutdown,
                settlement_task,
                telemetry_shutdown,
                telemetry_task,
                spool,
                monitor,
                shutdown_drain: settings.shutdown_drain,
            },
        ))
    }

    #[must_use]
    pub fn monitor(&self) -> RequestLogPipelineMonitor {
        self.monitor.clone()
    }

    pub async fn shutdown(self) {
        let Self {
            spool_shutdown,
            mut spool_task,
            spool_sync_shutdown,
            mut spool_sync_task,
            mut projection_task,
            settlement_shutdown,
            mut settlement_task,
            telemetry_shutdown,
            mut telemetry_task,
            spool,
            monitor,
            shutdown_drain,
        } = self;
        monitor.disconnect_projection_queue();
        let drain_deadline = Instant::now() + shutdown_drain + DATABASE_OPERATION_TIMEOUT;
        let _ = spool_shutdown.send(());
        await_or_abort(
            "spool_ingest",
            &mut spool_task,
            remaining_until(drain_deadline),
        )
        .await;
        let _ = spool_sync_shutdown.send(());
        await_or_abort(
            "spool_sync",
            &mut spool_sync_task,
            remaining_until(drain_deadline),
        )
        .await;
        await_or_abort(
            "projection",
            &mut projection_task,
            remaining_until(drain_deadline),
        )
        .await;
        let _ = settlement_shutdown.send(());
        await_or_abort(
            "settlement",
            &mut settlement_task,
            remaining_until(drain_deadline),
        )
        .await;
        let _ = telemetry_shutdown.send(());
        await_or_abort("telemetry", &mut telemetry_task, DATABASE_OPERATION_TIMEOUT).await;
        match timeout(DATABASE_OPERATION_TIMEOUT, sync_spool(Arc::clone(&spool))).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(%error, reason = "spool_sync_failed", "request-log spool final sync failed");
            }
            Err(_) => {
                tracing::error!(
                    reason = "spool_sync_timeout",
                    "request-log spool final sync timed out"
                );
            }
        }
        tracing::info!(
            spool_pending_bytes = spool.pending_bytes(),
            "durable request-log pipeline stopped"
        );
    }
}

async fn await_or_abort(name: &'static str, task: &mut JoinHandle<()>, duration: Duration) {
    match timeout(duration, &mut *task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::error!(worker = name, %error, "request-log worker terminated unexpectedly");
        }
        Err(_) => {
            tracing::warn!(
                worker = name,
                reason = "shutdown_timeout",
                "request-log worker exceeded its shutdown deadline and will be aborted"
            );
            task.abort();
            let _ = task.await;
        }
    }
}

#[derive(Clone)]
struct DurableRequestLogSettings {
    spool_directory: PathBuf,
    ingest_batch_size: usize,
    projection_batch_size: i64,
    settlement_batch_size: i64,
    settlement_interval: Duration,
    spool_sync_interval: Duration,
    spool_compaction_threshold_bytes: u64,
    database_max_connections: u32,
    metrics_interval: Option<Duration>,
    shutdown_drain: Duration,
}

impl From<&RequestLoggingConfig> for DurableRequestLogSettings {
    fn from(config: &RequestLoggingConfig) -> Self {
        Self {
            spool_directory: config.spool_directory.clone(),
            ingest_batch_size: config.ingest_batch_size,
            projection_batch_size: i64::try_from(config.projection_batch_size).unwrap_or(i64::MAX),
            settlement_batch_size: config.settlement_batch_size,
            settlement_interval: Duration::from_millis(config.settlement_interval_milliseconds),
            spool_sync_interval: Duration::from_millis(config.spool_sync_interval_milliseconds),
            spool_compaction_threshold_bytes: config.spool_compaction_threshold_bytes,
            database_max_connections: config.database_max_connections,
            metrics_interval: (config.metrics_interval_seconds > 0)
                .then(|| Duration::from_secs(config.metrics_interval_seconds)),
            shutdown_drain: Duration::from_secs(config.shutdown_drain_seconds),
        }
    }
}

struct SpoolIngestContext {
    repository: RequestLogRepository,
    spool: Arc<RequestLogSpool>,
    stage_sender: mpsc::Sender<()>,
    settings: DurableRequestLogSettings,
    metrics: Arc<RequestLogPipelineMetrics>,
}

async fn run_spool_ingest_worker(
    context: SpoolIngestContext,
    mut reader: SpoolReader,
    mut wake_receiver: mpsc::Receiver<()>,
    mut shutdown_requested: oneshot::Receiver<()>,
) {
    let SpoolIngestContext {
        repository,
        spool,
        stage_sender,
        settings,
        metrics,
    } = context;
    let mut drain_deadline = None;
    loop {
        if drain_deadline.is_none() {
            match shutdown_requested.try_recv() {
                Ok(()) | Err(oneshot::error::TryRecvError::Closed) => {
                    wake_receiver.close();
                    drain_deadline = Some(Instant::now() + settings.shutdown_drain);
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if deadline_reached(drain_deadline) {
            tracing::warn!(
                spool_pending_bytes = spool.pending_bytes(),
                reason = "shutdown_drain_timeout",
                "request-log spool retains durable rows for restart recovery"
            );
            let _ = sync_spool(Arc::clone(&spool)).await;
            return;
        }

        let batch = match reader.read_batch(settings.ingest_batch_size).await {
            Ok(batch) => batch,
            Err(error) => {
                metrics.record_ingress_failure();
                tracing::error!(
                    %error,
                    reason = "spool_read_failed",
                    "request-log spool ingestion stopped to preserve unread data"
                );
                return;
            }
        };
        if !batch.records.is_empty() {
            let started = Instant::now();
            let operation_timeout = remaining_operation_timeout(drain_deadline);
            let result = timeout(
                operation_timeout,
                repository.copy_ingest_batch(&batch.records),
            )
            .await;
            match result {
                Ok(Ok(rows)) => {
                    if rows != batch.records.len() as u64 {
                        metrics.record_ingress_failure();
                        tracing::error!(
                            expected_rows = batch.records.len(),
                            copied_rows = rows,
                            reason = "copy_row_count_mismatch",
                            "request-log ingress COPY returned an unexpected row count"
                        );
                        if let Err(error) = reader.reset(batch.start_offset).await {
                            tracing::error!(%error, "request-log spool reader reset failed");
                            return;
                        }
                        sleep(INGEST_RETRY_DELAY).await;
                        continue;
                    }
                    if let Err(error) = spool.checkpoint(batch.end_offset) {
                        metrics.record_ingress_failure();
                        tracing::error!(
                            %error,
                            reason = "checkpoint_failed",
                            "request-log ingress committed but spool checkpoint failed; replay is safe"
                        );
                        if let Err(reset_error) = reader.reset(batch.start_offset).await {
                            tracing::error!(%reset_error, "request-log spool reader reset failed");
                            return;
                        }
                        sleep(INGEST_RETRY_DELAY).await;
                        continue;
                    }
                    metrics.record_ingress_batch(rows, duration_micros(started.elapsed()));
                    match stage_sender.try_send(()) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            tracing::warn!(
                                reason = "projection_worker_closed",
                                "request logs remain durable in the database ingress table"
                            );
                        }
                    }
                    match compact_spool(Arc::clone(&spool)).await {
                        Ok(true) => {
                            if let Err(error) = reader.reset(0).await {
                                tracing::error!(%error, "compacted spool reader reset failed");
                                return;
                            }
                        }
                        Ok(false) => {}
                        Err(error) => {
                            tracing::warn!(%error, "request-log spool compaction failed");
                        }
                    }
                    continue;
                }
                Ok(Err(error)) => {
                    metrics.record_ingress_failure();
                    tracing::error!(
                        %error,
                        event_count = batch.records.len(),
                        reason = "ingress_copy_failed",
                        "request-log ingress COPY failed; the local spool will retry"
                    );
                }
                Err(_) => {
                    metrics.record_ingress_failure();
                    tracing::error!(
                        event_count = batch.records.len(),
                        reason = "ingress_copy_timeout",
                        "request-log ingress COPY timed out; the local spool will retry"
                    );
                }
            }
            if let Err(error) = reader.reset(batch.start_offset).await {
                tracing::error!(%error, "request-log spool reader reset failed");
                return;
            }
            sleep(INGEST_RETRY_DELAY).await;
            continue;
        }

        if drain_deadline.is_some() {
            let _ = sync_spool(Arc::clone(&spool)).await;
            return;
        }
        tokio::select! {
            _ = &mut shutdown_requested => {
                wake_receiver.close();
                drain_deadline = Some(Instant::now() + settings.shutdown_drain);
            }
            _ = wake_receiver.recv() => {}
        }
    }
}

async fn run_spool_sync_worker(
    spool: Arc<RequestLogSpool>,
    frequency: Duration,
    mut shutdown_requested: oneshot::Receiver<()>,
) {
    let mut ticker = interval(frequency);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = sync_spool(Arc::clone(&spool)).await {
                    tracing::error!(%error, reason = "spool_sync_failed", "request-log spool sync failed");
                }
            }
            _ = &mut shutdown_requested => return,
        }
    }
}

async fn run_projection_worker(
    repository: RequestLogRepository,
    mut stage_receiver: mpsc::Receiver<()>,
    settings: DurableRequestLogSettings,
    metrics: Arc<RequestLogPipelineMetrics>,
) {
    let mut source_closed = false;
    let mut poll = interval(PROJECTION_POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        match project_one_batch(&repository, &settings, &metrics).await {
            ProjectionProgress::Progress => continue,
            ProjectionProgress::Idle if source_closed => return,
            ProjectionProgress::Idle => {}
        }
        tokio::select! {
            signal = stage_receiver.recv() => {
                if signal.is_none() {
                    source_closed = true;
                }
            }
            _ = poll.tick() => {}
        }
    }
}

enum ProjectionProgress {
    Progress,
    Idle,
}

async fn project_one_batch(
    repository: &RequestLogRepository,
    settings: &DurableRequestLogSettings,
    metrics: &RequestLogPipelineMetrics,
) -> ProjectionProgress {
    let rows = match timeout(
        DATABASE_OPERATION_TIMEOUT,
        repository.load_ingest_batch(settings.projection_batch_size),
    )
    .await
    {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => {
            metrics.record_projection_failure();
            tracing::error!(%error, reason = "ingest_load_failed", "request-log projection load failed");
            sleep(INGEST_RETRY_DELAY).await;
            return ProjectionProgress::Idle;
        }
        Err(_) => {
            metrics.record_projection_failure();
            tracing::error!(
                reason = "ingest_load_timeout",
                "request-log projection load timed out"
            );
            return ProjectionProgress::Idle;
        }
    };
    if rows.is_empty() {
        return ProjectionProgress::Idle;
    }

    let started = Instant::now();
    let mut valid_rows = Vec::with_capacity(rows.len());
    let mut invalid_sequences = Vec::new();
    for row in rows {
        match row.encoded().decode() {
            Ok(event) => valid_rows.push((row, event)),
            Err(error) => {
                metrics.record_projection_failure();
                tracing::error!(
                    sequence = row.sequence,
                    request_log_id = %row.request_log_id,
                    %error,
                    reason = "ingest_decode_failed",
                    "request-log ingress row could not be decoded"
                );
                invalid_sequences.push(row.sequence);
            }
        }
    }
    defer_rows(
        repository,
        &invalid_sequences,
        "decode_failed",
        ISOLATED_PROJECTION_RETRY_SECONDS,
        metrics,
    )
    .await;
    if valid_rows.is_empty() {
        metrics.record_projection(
            0,
            invalid_sequences.len() as u64,
            duration_micros(started.elapsed()),
        );
        return ProjectionProgress::Progress;
    }

    let events = valid_rows
        .iter()
        .map(|(_, event)| event.clone())
        .collect::<Vec<_>>();
    match timeout(DATABASE_OPERATION_TIMEOUT, repository.insert_batch(&events)).await {
        Ok(Ok(results)) => {
            let mut acknowledged = Vec::with_capacity(results.len());
            let mut conflicting = Vec::new();
            let mut invalid = Vec::new();
            for ((row, _), result) in valid_rows.iter().zip(results) {
                match result.outcome {
                    RequestLogBatchInsertOutcome::Inserted
                    | RequestLogBatchInsertOutcome::ExactDuplicate => {
                        acknowledged.push(row.sequence);
                    }
                    RequestLogBatchInsertOutcome::DuplicateConflict => {
                        conflicting.push(row.sequence);
                        tracing::error!(
                            sequence = row.sequence,
                            request_log_id = %row.request_log_id,
                            reason = "duplicate_conflict",
                            "request-log ingress row conflicts with immutable final facts"
                        );
                    }
                    RequestLogBatchInsertOutcome::InvalidResponseStatus { status } => {
                        invalid.push(row.sequence);
                        tracing::error!(
                            sequence = row.sequence,
                            request_log_id = %row.request_log_id,
                            status,
                            reason = "invalid_response_status",
                            "request-log ingress row has an invalid response status"
                        );
                    }
                }
            }
            acknowledge_rows(repository, &acknowledged, metrics).await;
            defer_rows(
                repository,
                &conflicting,
                "duplicate_conflict",
                ISOLATED_PROJECTION_RETRY_SECONDS,
                metrics,
            )
            .await;
            defer_rows(
                repository,
                &invalid,
                "invalid_response_status",
                ISOLATED_PROJECTION_RETRY_SECONDS,
                metrics,
            )
            .await;
            metrics.record_projection(
                acknowledged.len() as u64,
                (invalid_sequences.len() + conflicting.len() + invalid.len()) as u64,
                duration_micros(started.elapsed()),
            );
        }
        Ok(Err(error)) => {
            metrics.record_projection_failure();
            if valid_rows.iter().any(|(row, _)| row.attempt_count > 0) {
                project_rows_individually(repository, &valid_rows, metrics).await;
            } else {
                let sequences = valid_rows
                    .iter()
                    .map(|(row, _)| row.sequence)
                    .collect::<Vec<_>>();
                tracing::error!(
                    %error,
                    event_count = sequences.len(),
                    reason = "projection_batch_failed",
                    "request-log projection batch failed and will retry"
                );
                defer_rows(
                    repository,
                    &sequences,
                    "batch_insert_failed",
                    FIRST_PROJECTION_RETRY_SECONDS,
                    metrics,
                )
                .await;
                metrics.record_projection(
                    0,
                    sequences.len() as u64,
                    duration_micros(started.elapsed()),
                );
            }
        }
        Err(_) => {
            metrics.record_projection_failure();
            let sequences = valid_rows
                .iter()
                .map(|(row, _)| row.sequence)
                .collect::<Vec<_>>();
            tracing::error!(
                event_count = sequences.len(),
                reason = "projection_batch_timeout",
                "request-log projection batch timed out and will retry"
            );
            defer_rows(
                repository,
                &sequences,
                "batch_insert_timeout",
                FIRST_PROJECTION_RETRY_SECONDS,
                metrics,
            )
            .await;
            metrics.record_projection(
                0,
                sequences.len() as u64,
                duration_micros(started.elapsed()),
            );
        }
    }
    ProjectionProgress::Progress
}

async fn project_rows_individually(
    repository: &RequestLogRepository,
    rows: &[(RequestLogIngestRecord, crate::domain::RequestLogEvent)],
    metrics: &RequestLogPipelineMetrics,
) {
    let started = Instant::now();
    let mut acknowledged = Vec::new();
    let mut deferred = Vec::new();
    for (row, event) in rows {
        match timeout(DATABASE_OPERATION_TIMEOUT, repository.insert(event)).await {
            Ok(Ok(_)) => acknowledged.push(row.sequence),
            Ok(Err(error)) => {
                tracing::error!(
                    sequence = row.sequence,
                    request_log_id = %row.request_log_id,
                    %error,
                    reason = "isolated_projection_failed",
                    "request-log ingress row remains durable for a later retry"
                );
                deferred.push(row.sequence);
            }
            Err(_) => {
                tracing::error!(
                    sequence = row.sequence,
                    request_log_id = %row.request_log_id,
                    reason = "isolated_projection_timeout",
                    "request-log ingress row remains durable for a later retry"
                );
                deferred.push(row.sequence);
            }
        }
    }
    acknowledge_rows(repository, &acknowledged, metrics).await;
    defer_rows(
        repository,
        &deferred,
        "isolated_insert_failed",
        ISOLATED_PROJECTION_RETRY_SECONDS,
        metrics,
    )
    .await;
    metrics.record_projection(
        acknowledged.len() as u64,
        deferred.len() as u64,
        duration_micros(started.elapsed()),
    );
}

async fn acknowledge_rows(
    repository: &RequestLogRepository,
    sequences: &[i64],
    metrics: &RequestLogPipelineMetrics,
) {
    if sequences.is_empty() {
        return;
    }
    match timeout(
        DATABASE_OPERATION_TIMEOUT,
        repository.acknowledge_ingest(sequences),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            metrics.record_projection_failure();
            tracing::error!(
                %error,
                event_count = sequences.len(),
                reason = "ingest_acknowledge_failed",
                "projected rows remain in ingress and may be replayed idempotently"
            );
        }
        Err(_) => {
            metrics.record_projection_failure();
            tracing::error!(
                event_count = sequences.len(),
                reason = "ingest_acknowledge_timeout",
                "projected rows remain in ingress and may be replayed idempotently"
            );
        }
    }
}

async fn defer_rows(
    repository: &RequestLogRepository,
    sequences: &[i64],
    error_code: &str,
    retry_after_seconds: i64,
    metrics: &RequestLogPipelineMetrics,
) {
    if sequences.is_empty() {
        return;
    }
    match timeout(
        DATABASE_OPERATION_TIMEOUT,
        repository.defer_ingest(sequences, error_code, retry_after_seconds),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            metrics.record_projection_failure();
            tracing::error!(
                %error,
                event_count = sequences.len(),
                reason = "ingest_defer_failed",
                "failed ingress rows could not be rescheduled"
            );
        }
        Err(_) => {
            metrics.record_projection_failure();
            tracing::error!(
                event_count = sequences.len(),
                reason = "ingest_defer_timeout",
                "failed ingress rows could not be rescheduled"
            );
        }
    }
}

async fn run_durable_settlement_worker(
    repository: RequestLogRepository,
    mut shutdown_requested: oneshot::Receiver<()>,
    settings: DurableRequestLogSettings,
    admission: Option<AdmissionRuntime>,
    metrics: Arc<RequestLogPipelineMetrics>,
) {
    let mut ticker = interval(settings.settlement_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                settle_one_durable_batch(
                    &repository,
                    settings.settlement_batch_size,
                    admission.as_ref(),
                    &metrics,
                ).await;
            }
            _ = &mut shutdown_requested => {
                drain_settlements(
                    &repository,
                    settings.settlement_batch_size,
                    settings.shutdown_drain,
                    admission.as_ref(),
                    &metrics,
                ).await;
                return;
            }
        }
    }
}

async fn settle_one_durable_batch(
    repository: &RequestLogRepository,
    batch_size: i64,
    admission: Option<&AdmissionRuntime>,
    metrics: &RequestLogPipelineMetrics,
) -> usize {
    let started = Instant::now();
    match timeout(
        DATABASE_OPERATION_TIMEOUT,
        repository.settle_pending(batch_size),
    )
    .await
    {
        Ok(Ok(outcomes)) => {
            let count = outcomes.len();
            let settled = outcomes
                .into_iter()
                .map(|outcome| {
                    let settled = matches!(outcome, RequestLogSettlementOutcome::Settled { .. });
                    super::handle_settlement_outcome(None, outcome, admission);
                    u64::from(settled)
                })
                .sum();
            metrics.record_settlement(settled, duration_micros(started.elapsed()));
            count
        }
        Ok(Err(error)) => {
            metrics.record_settlement_failure();
            tracing::error!(%error, reason = "settlement_failed", "request-log settlement batch failed");
            0
        }
        Err(_) => {
            metrics.record_settlement_failure();
            tracing::error!(
                reason = "settlement_timeout",
                "request-log settlement batch timed out"
            );
            0
        }
    }
}

async fn drain_settlements(
    repository: &RequestLogRepository,
    batch_size: i64,
    drain_duration: Duration,
    admission: Option<&AdmissionRuntime>,
    metrics: &RequestLogPipelineMetrics,
) {
    let deadline = Instant::now() + drain_duration;
    loop {
        if Instant::now() >= deadline {
            tracing::warn!(
                reason = "settlement_shutdown_timeout",
                "unsettled durable request logs remain for restart recovery"
            );
            return;
        }
        let count = settle_one_durable_batch(repository, batch_size, admission, metrics).await;
        if count < batch_size.max(1) as usize {
            return;
        }
        tokio::task::yield_now().await;
    }
}

async fn run_telemetry_reporter(
    repository: RequestLogRepository,
    spool: Arc<RequestLogSpool>,
    mut shutdown_requested: oneshot::Receiver<()>,
    settings: DurableRequestLogSettings,
    metrics: Arc<RequestLogPipelineMetrics>,
) {
    let poll_interval = settings
        .metrics_interval
        .map_or(TELEMETRY_CHECK_INTERVAL, |interval| {
            interval.min(TELEMETRY_CHECK_INTERVAL)
        });
    let mut ticker = interval(poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker.tick().await;
    let mut state = RequestLogTelemetryState::default();
    let mut next_metrics_at = settings
        .metrics_interval
        .map(|interval| Instant::now() + interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let sample = load_telemetry_sample(
                    &repository,
                    &spool,
                    &metrics,
                    settings.database_max_connections,
                ).await;
                let now = Instant::now();
                emit_telemetry_transitions(&state.observe(&sample, now), &sample);
                if metrics_heartbeat_due(
                    &mut next_metrics_at,
                    settings.metrics_interval,
                    now,
                ) {
                    emit_metrics(&sample);
                }
            }
            _ = &mut shutdown_requested => {
                if settings.metrics_interval.is_some() {
                    let sample = load_telemetry_sample(
                        &repository,
                        &spool,
                        &metrics,
                        settings.database_max_connections,
                    ).await;
                    emit_metrics(&sample);
                }
                return;
            }
        }
    }
}

async fn load_telemetry_sample(
    repository: &RequestLogRepository,
    spool: &RequestLogSpool,
    metrics: &RequestLogPipelineMetrics,
    database_pool_capacity: u32,
) -> RequestLogTelemetrySample {
    let sampled_at = Utc::now();
    // Capture pressure before issuing the two health queries. SQLx returns
    // dropped pool connections asynchronously, so sampling afterward can
    // briefly count this probe's own connections as busy.
    let pool_before_queries = repository.pool_status();
    let (ingress, settlement) = tokio::join!(
        timeout(DATABASE_OPERATION_TIMEOUT, repository.ingest_backlog()),
        timeout(DATABASE_OPERATION_TIMEOUT, repository.settlement_backlog())
    );
    RequestLogTelemetrySample {
        metrics: metrics.snapshot(),
        spool_pending_bytes: spool.pending_bytes(),
        ingress: match ingress {
            Ok(Ok(backlog)) => BacklogSample::Available(backlog_health(
                backlog.row_count,
                backlog.oldest_staged_at,
                sampled_at,
            )),
            Ok(Err(error)) => BacklogSample::Unavailable {
                error: error.to_string(),
            },
            Err(_) => BacklogSample::Unavailable {
                error: "query timed out".into(),
            },
        },
        settlement: match settlement {
            Ok(Ok(backlog)) => BacklogSample::Available(backlog_health(
                backlog.row_count,
                backlog.oldest_completed_at,
                sampled_at,
            )),
            Ok(Err(error)) => BacklogSample::Unavailable {
                error: error.to_string(),
            },
            Err(_) => BacklogSample::Unavailable {
                error: "query timed out".into(),
            },
        },
        database_pool_size: pool_before_queries.size,
        database_pool_idle: pool_before_queries.idle,
        database_pool_capacity,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BacklogHealth {
    row_count: u64,
    oldest_age_seconds: u64,
}

#[derive(Debug)]
enum BacklogSample {
    Available(BacklogHealth),
    Unavailable { error: String },
}

impl BacklogSample {
    const fn health(&self) -> Option<BacklogHealth> {
        match self {
            Self::Available(health) => Some(*health),
            Self::Unavailable { .. } => None,
        }
    }

    const fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    fn error(&self) -> Option<&str> {
        match self {
            Self::Available(_) => None,
            Self::Unavailable { error } => Some(error),
        }
    }
}

#[derive(Debug)]
struct RequestLogTelemetrySample {
    metrics: RequestLogPipelineMetricsSnapshot,
    spool_pending_bytes: u64,
    ingress: BacklogSample,
    settlement: BacklogSample,
    database_pool_size: u32,
    database_pool_idle: usize,
    database_pool_capacity: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TelemetryTransition {
    IngressQueryUnavailable,
    IngressQueryRecovered,
    IngressBacklogStalled,
    IngressBacklogRecovered,
    SettlementQueryUnavailable,
    SettlementQueryRecovered,
    SettlementBacklogStalled,
    SettlementBacklogRecovered,
    DatabasePoolSaturated,
    DatabasePoolRecovered,
}

#[derive(Debug, Default)]
struct RequestLogTelemetryState {
    ingress_query_unavailable: bool,
    ingress_backlog_stalled: bool,
    settlement_query_unavailable: bool,
    settlement_backlog_stalled: bool,
    database_pool_saturated_since: Option<Instant>,
    database_pool_saturated: bool,
}

impl RequestLogTelemetryState {
    fn observe(
        &mut self,
        sample: &RequestLogTelemetrySample,
        now: Instant,
    ) -> Vec<TelemetryTransition> {
        let mut transitions = Vec::new();
        observe_backlog(
            sample.ingress.health(),
            &mut self.ingress_query_unavailable,
            &mut self.ingress_backlog_stalled,
            TelemetryTransition::IngressQueryUnavailable,
            TelemetryTransition::IngressQueryRecovered,
            TelemetryTransition::IngressBacklogStalled,
            TelemetryTransition::IngressBacklogRecovered,
            &mut transitions,
        );
        observe_backlog(
            sample.settlement.health(),
            &mut self.settlement_query_unavailable,
            &mut self.settlement_backlog_stalled,
            TelemetryTransition::SettlementQueryUnavailable,
            TelemetryTransition::SettlementQueryRecovered,
            TelemetryTransition::SettlementBacklogStalled,
            TelemetryTransition::SettlementBacklogRecovered,
            &mut transitions,
        );

        let pool_saturated = sample.database_pool_capacity > 0
            && sample.database_pool_size >= sample.database_pool_capacity
            && sample.database_pool_idle == 0;
        if pool_saturated {
            let saturated_since = self.database_pool_saturated_since.get_or_insert(now);
            if !self.database_pool_saturated
                && now.saturating_duration_since(*saturated_since) >= DATABASE_POOL_SATURATED_AFTER
            {
                self.database_pool_saturated = true;
                transitions.push(TelemetryTransition::DatabasePoolSaturated);
            }
        } else {
            self.database_pool_saturated_since = None;
            if std::mem::take(&mut self.database_pool_saturated) {
                transitions.push(TelemetryTransition::DatabasePoolRecovered);
            }
        }
        transitions
    }
}

#[allow(clippy::too_many_arguments)]
fn observe_backlog(
    health: Option<BacklogHealth>,
    query_unavailable: &mut bool,
    backlog_stalled: &mut bool,
    unavailable: TelemetryTransition,
    query_recovered: TelemetryTransition,
    stalled: TelemetryTransition,
    backlog_recovered: TelemetryTransition,
    transitions: &mut Vec<TelemetryTransition>,
) {
    let Some(health) = health else {
        if !std::mem::replace(query_unavailable, true) {
            transitions.push(unavailable);
        }
        return;
    };

    if std::mem::take(query_unavailable) {
        transitions.push(query_recovered);
    }
    let is_stalled =
        health.row_count > 0 && health.oldest_age_seconds >= BACKLOG_STALE_AFTER.as_secs();
    match (is_stalled, *backlog_stalled) {
        (true, false) => {
            *backlog_stalled = true;
            transitions.push(stalled);
        }
        (false, true) => {
            *backlog_stalled = false;
            transitions.push(backlog_recovered);
        }
        _ => {}
    }
}

fn emit_telemetry_transitions(
    transitions: &[TelemetryTransition],
    sample: &RequestLogTelemetrySample,
) {
    for transition in transitions {
        match transition {
            TelemetryTransition::IngressQueryUnavailable => {
                tracing::warn!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_ingress_query_unavailable",
                    error = sample.ingress.error().unwrap_or("unknown error"),
                    "request-log ingress backlog telemetry is unavailable"
                );
            }
            TelemetryTransition::IngressQueryRecovered => {
                tracing::info!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_ingress_query_recovered",
                    "request-log ingress backlog telemetry recovered"
                );
            }
            TelemetryTransition::IngressBacklogStalled => {
                let backlog = sample.ingress.health().unwrap_or_default();
                tracing::warn!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_ingress_backlog_stalled",
                    backlog_rows_estimate = backlog.row_count,
                    oldest_age_seconds = backlog.oldest_age_seconds,
                    spool_pending_bytes = sample.spool_pending_bytes,
                    stale_after_seconds = BACKLOG_STALE_AFTER.as_secs(),
                    "request-log ingress backlog is not draining within the health threshold"
                );
            }
            TelemetryTransition::IngressBacklogRecovered => {
                tracing::info!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_ingress_backlog_recovered",
                    "request-log ingress backlog recovered"
                );
            }
            TelemetryTransition::SettlementQueryUnavailable => {
                tracing::warn!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_settlement_query_unavailable",
                    error = sample.settlement.error().unwrap_or("unknown error"),
                    "request-log settlement backlog telemetry is unavailable"
                );
            }
            TelemetryTransition::SettlementQueryRecovered => {
                tracing::info!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_settlement_query_recovered",
                    "request-log settlement backlog telemetry recovered"
                );
            }
            TelemetryTransition::SettlementBacklogStalled => {
                let backlog = sample.settlement.health().unwrap_or_default();
                tracing::warn!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_settlement_backlog_stalled",
                    backlog_rows = backlog.row_count,
                    oldest_age_seconds = backlog.oldest_age_seconds,
                    stale_after_seconds = BACKLOG_STALE_AFTER.as_secs(),
                    "request-log settlement backlog is not draining within the health threshold"
                );
            }
            TelemetryTransition::SettlementBacklogRecovered => {
                tracing::info!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_settlement_backlog_recovered",
                    "request-log settlement backlog recovered"
                );
            }
            TelemetryTransition::DatabasePoolSaturated => {
                tracing::warn!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_database_pool_saturated",
                    database_pool_size = sample.database_pool_size,
                    database_pool_idle = sample.database_pool_idle,
                    database_pool_capacity = sample.database_pool_capacity,
                    saturated_after_seconds = DATABASE_POOL_SATURATED_AFTER.as_secs(),
                    "request-log database pool remains saturated"
                );
            }
            TelemetryTransition::DatabasePoolRecovered => {
                tracing::info!(
                    target: "ai_gateway::request_log_health",
                    event = "request_log_database_pool_recovered",
                    database_pool_size = sample.database_pool_size,
                    database_pool_idle = sample.database_pool_idle,
                    database_pool_capacity = sample.database_pool_capacity,
                    "request-log database pool recovered"
                );
            }
        }
    }
}

fn emit_metrics(sample: &RequestLogTelemetrySample) {
    let snapshot = sample.metrics;
    let ingress = sample.ingress.health().unwrap_or_default();
    let settlement = sample.settlement.health().unwrap_or_default();
    tracing::info!(
        target: "ai_gateway::request_log_metrics",
        event = "request_log_metrics",
        recorded_total = snapshot.recorded_total,
        spooled_total = snapshot.spooled_total,
        spool_append_failures_total = snapshot.spool_append_failures_total,
        spool_bytes_total = snapshot.spool_bytes_total,
        spool_pending_bytes = sample.spool_pending_bytes,
        ingress_batches_total = snapshot.ingress_batches_total,
        ingress_rows_total = snapshot.ingress_rows_total,
        ingress_failures_total = snapshot.ingress_failures_total,
        ingress_duration_micros_total = snapshot.ingress_duration_micros_total,
        ingress_duration_micros_max = snapshot.ingress_duration_micros_max,
        ingress_backlog_query_available = sample.ingress.is_available(),
        ingress_backlog_rows_estimate = ingress.row_count,
        oldest_ingress_age_seconds = ingress.oldest_age_seconds,
        projected_rows_total = snapshot.projected_rows_total,
        projection_deferred_total = snapshot.projection_deferred_total,
        projection_failures_total = snapshot.projection_failures_total,
        projection_duration_micros_total = snapshot.projection_duration_micros_total,
        projection_duration_micros_max = snapshot.projection_duration_micros_max,
        settled_rows_total = snapshot.settled_rows_total,
        settlement_failures_total = snapshot.settlement_failures_total,
        settlement_duration_micros_total = snapshot.settlement_duration_micros_total,
        settlement_duration_micros_max = snapshot.settlement_duration_micros_max,
        settlement_backlog_query_available = sample.settlement.is_available(),
        settlement_backlog_rows = settlement.row_count,
        settlement_oldest_age_seconds = settlement.oldest_age_seconds,
        database_pool_size = sample.database_pool_size,
        database_pool_idle = sample.database_pool_idle,
        database_pool_capacity = sample.database_pool_capacity,
        "request-log pipeline metrics"
    );
}

fn metrics_heartbeat_due(
    next_metrics_at: &mut Option<Instant>,
    interval: Option<Duration>,
    now: Instant,
) -> bool {
    let (Some(mut next), Some(interval)) = (*next_metrics_at, interval) else {
        return false;
    };
    if now < next {
        return false;
    }
    while next <= now {
        next += interval;
    }
    *next_metrics_at = Some(next);
    true
}

fn backlog_health(
    row_count: i64,
    oldest_at: Option<DateTime<Utc>>,
    sampled_at: DateTime<Utc>,
) -> BacklogHealth {
    BacklogHealth {
        row_count: u64::try_from(row_count.max(0)).unwrap_or(u64::MAX),
        oldest_age_seconds: oldest_at.map_or(0, |oldest| {
            u64::try_from((sampled_at - oldest).num_seconds().max(0)).unwrap_or(u64::MAX)
        }),
    }
}

fn deadline_reached(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

fn remaining_operation_timeout(deadline: Option<Instant>) -> Duration {
    deadline.map_or(DATABASE_OPERATION_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .min(DATABASE_OPERATION_TIMEOUT)
    })
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

async fn sync_spool(spool: Arc<RequestLogSpool>) -> Result<(), SpoolTaskError> {
    tokio::task::spawn_blocking(move || spool.sync_data())
        .await?
        .map_err(SpoolTaskError::from)
}

async fn compact_spool(spool: Arc<RequestLogSpool>) -> Result<bool, SpoolTaskError> {
    tokio::task::spawn_blocking(move || spool.compact_if_drained())
        .await?
        .map_err(SpoolTaskError::from)
}

#[cfg(test)]
mod telemetry_tests {
    use super::{
        BACKLOG_STALE_AFTER, BacklogHealth, BacklogSample, DurableRequestLogSettings,
        RequestLogTelemetrySample, RequestLogTelemetryState, TelemetryTransition,
        metrics_heartbeat_due,
    };
    use crate::{
        observability::RequestLogPipelineMetricsSnapshot, runtime_config::RequestLoggingConfig,
    };
    use std::time::Duration;
    use tokio::time::Instant;

    fn sample() -> RequestLogTelemetrySample {
        RequestLogTelemetrySample {
            metrics: RequestLogPipelineMetricsSnapshot::default(),
            spool_pending_bytes: 0,
            ingress: BacklogSample::Available(BacklogHealth::default()),
            settlement: BacklogSample::Available(BacklogHealth::default()),
            database_pool_size: 3,
            database_pool_idle: 3,
            database_pool_capacity: 4,
        }
    }

    #[test]
    fn stale_backlog_logs_once_and_then_recovers() {
        let now = Instant::now();
        let mut state = RequestLogTelemetryState::default();
        let mut sample = sample();
        sample.ingress = BacklogSample::Available(BacklogHealth {
            row_count: 7,
            oldest_age_seconds: BACKLOG_STALE_AFTER.as_secs(),
        });

        assert_eq!(
            state.observe(&sample, now),
            vec![TelemetryTransition::IngressBacklogStalled]
        );
        assert!(state.observe(&sample, now).is_empty());

        sample.ingress = BacklogSample::Available(BacklogHealth::default());
        assert_eq!(
            state.observe(&sample, now),
            vec![TelemetryTransition::IngressBacklogRecovered]
        );
    }

    #[test]
    fn backlog_query_outage_and_recovery_are_transition_based() {
        let now = Instant::now();
        let mut state = RequestLogTelemetryState::default();
        let mut sample = sample();
        sample.settlement = BacklogSample::Unavailable {
            error: "database unavailable".into(),
        };

        assert_eq!(
            state.observe(&sample, now),
            vec![TelemetryTransition::SettlementQueryUnavailable]
        );
        assert!(state.observe(&sample, now).is_empty());

        sample.settlement = BacklogSample::Available(BacklogHealth::default());
        assert_eq!(
            state.observe(&sample, now),
            vec![TelemetryTransition::SettlementQueryRecovered]
        );
    }

    #[test]
    fn database_pool_requires_sustained_saturation() {
        let started = Instant::now();
        let mut state = RequestLogTelemetryState::default();
        let mut sample = sample();
        sample.database_pool_size = 4;
        sample.database_pool_idle = 0;

        assert!(state.observe(&sample, started).is_empty());
        assert!(
            state
                .observe(
                    &sample,
                    started + super::DATABASE_POOL_SATURATED_AFTER - Duration::from_secs(1),
                )
                .is_empty()
        );
        assert_eq!(
            state.observe(&sample, started + super::DATABASE_POOL_SATURATED_AFTER),
            vec![TelemetryTransition::DatabasePoolSaturated]
        );
        assert!(
            state
                .observe(&sample, started + super::DATABASE_POOL_SATURATED_AFTER)
                .is_empty()
        );

        sample.database_pool_idle = 1;
        assert_eq!(
            state.observe(&sample, started + super::DATABASE_POOL_SATURATED_AFTER),
            vec![TelemetryTransition::DatabasePoolRecovered]
        );
    }

    #[test]
    fn database_pool_with_idle_headroom_never_enters_saturation() {
        let started = Instant::now();
        let mut state = RequestLogTelemetryState::default();
        let mut sample = sample();
        sample.database_pool_size = 4;
        sample.database_pool_idle = 2;

        assert!(state.observe(&sample, started).is_empty());
        assert!(
            state
                .observe(&sample, started + super::DATABASE_POOL_SATURATED_AFTER,)
                .is_empty()
        );
    }

    #[test]
    fn periodic_metrics_are_disabled_by_default() {
        let config = RequestLoggingConfig::default();
        let settings = DurableRequestLogSettings::from(&config);
        assert!(settings.metrics_interval.is_none());

        let now = Instant::now();
        let mut next = None;
        assert!(!metrics_heartbeat_due(&mut next, None, now));
    }

    #[test]
    fn configured_metrics_heartbeat_advances_after_emission() {
        let interval = Duration::from_secs(300);
        let started = Instant::now();
        let mut next = Some(started + interval);
        assert!(!metrics_heartbeat_due(
            &mut next,
            Some(interval),
            started + Duration::from_secs(299)
        ));
        assert!(metrics_heartbeat_due(
            &mut next,
            Some(interval),
            started + interval
        ));
        assert_eq!(next, Some(started + interval + interval));
    }
}

#[derive(Debug, Error)]
enum SpoolTaskError {
    #[error("request-log spool operation failed: {message}")]
    Spool { message: String },
    #[error("request-log spool task failed")]
    Join(#[from] JoinError),
}

impl From<crate::request_log_spool::SpoolError> for SpoolTaskError {
    fn from(error: crate::request_log_spool::SpoolError) -> Self {
        Self::Spool {
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Error)]
pub enum DurableRequestLogWorkerStartError {
    #[error("request-log spool initialization failed: {message}")]
    Spool { message: String },
    #[error("request-log spool initialization task failed")]
    Join(#[from] JoinError),
}
