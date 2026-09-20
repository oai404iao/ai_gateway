//! Explicit backend dispatch for the durable request-log pipeline: durable
//! ingress, immutable financial facts, the independent query-log projection,
//! and single-transaction settlement.
//!
//! Every operation is implemented by both backends. Derived handles clone the same
//! database handle, so a SQLite facade never opens an additional pool.

use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::RequestLogEvent;

use super::RepositoryError;
use super::backend_queries::RequestLogQueries;
use super::metering::{
    MeteringReconciliationCounts, MeteringWriteOutcome, PostgresMeteringRepository,
};
use super::postgres_control_plane::{
    IngestReceipt, PostgresRequestLogRepository, PostgresSettlementRepository,
    RequestLogBatchInsertResult, RequestLogIngestBacklog, RequestLogIngestRecord,
    RequestLogInsertOutcome, RequestLogPoolStatus, RequestLogSettlementBacklog,
    RequestLogSettlementOutcome,
};

#[cfg(feature = "sqlite-backend")]
use std::sync::Arc;

#[cfg(feature = "sqlite-backend")]
use super::sqlite::{
    SqliteDatabase, SqliteMeteringRepository, SqliteRequestLogRepository,
    SqliteSettlementRepository,
};

#[derive(Clone)]
enum LogsBackend {
    Postgres(PostgresRequestLogRepository),
    #[cfg(feature = "sqlite-backend")]
    Sqlite {
        repository: SqliteRequestLogRepository,
        database: Arc<SqliteDatabase>,
    },
}

/// Durable ingress plus the idempotent query-log projection.
#[derive(Clone)]
pub struct RequestLogRepository {
    backend: LogsBackend,
}

impl RequestLogRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: LogsBackend::Postgres(PostgresRequestLogRepository::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: LogsBackend::Sqlite {
                repository: SqliteRequestLogRepository::new(Arc::clone(&database)),
                database,
            },
        }
    }

    /// Console read surface over the same backend and database handle.
    #[must_use]
    pub fn queries(&self) -> RequestLogQueries {
        match &self.backend {
            LogsBackend::Postgres(repository) => {
                RequestLogQueries::from_postgres(repository.queries())
            }
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { database, .. } => {
                RequestLogQueries::from_sqlite(Arc::clone(database))
            }
        }
    }

    /// Settlement claims over the same backend and database handle.
    #[must_use]
    pub fn settlements(&self) -> SettlementRepository {
        match &self.backend {
            LogsBackend::Postgres(repository) => {
                SettlementRepository::from_postgres(repository.settlements())
            }
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { database, .. } => {
                SettlementRepository::from_sqlite(Arc::clone(database))
            }
        }
    }

    /// Immutable financial facts over the same backend and database handle.
    #[must_use]
    pub fn metering(&self) -> MeteringRepository {
        match &self.backend {
            LogsBackend::Postgres(repository) => {
                MeteringRepository::from_postgres(repository.metering())
            }
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { database, .. } => {
                MeteringRepository::from_sqlite(Arc::clone(database))
            }
        }
    }

    /// Appends encoded terminal events to the low-index durable ingress table.
    ///
    /// Duplicates are intentional: a checkpoint replay may re-send rows, while
    /// the final query-log primary key remains the idempotency boundary.
    pub(crate) async fn accept_batch(
        &self,
        records: &[crate::request_log_journal::EncodedRequestLog],
    ) -> Result<u64, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.accept_batch(records).await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.accept_batch(records).await,
        }
    }

    /// Oldest-first slice of ingress rows that still need financial facts.
    pub(crate) async fn load_ingest_batch(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogIngestRecord>, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.load_ingest_batch(limit).await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.load_ingest_batch(limit).await,
        }
    }

    /// Removes projected ingress rows once both independent stages are durable.
    pub(crate) async fn acknowledge_ingest(
        &self,
        sequences: &[IngestReceipt],
    ) -> Result<u64, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.acknowledge_ingest(sequences).await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => {
                repository.acknowledge_ingest(sequences).await
            }
        }
    }

    /// Reschedules failed projection work with its independent retry budget.
    pub(crate) async fn defer_ingest(
        &self,
        sequences: &[IngestReceipt],
        error_code: &str,
        retry_after_seconds: i64,
    ) -> Result<u64, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => {
                repository
                    .defer_ingest(sequences, error_code, retry_after_seconds)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => {
                repository
                    .defer_ingest(sequences, error_code, retry_after_seconds)
                    .await
            }
        }
    }

    pub(crate) async fn ingest_backlog(&self) -> Result<RequestLogIngestBacklog, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.ingest_backlog().await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.ingest_backlog().await,
        }
    }

    pub(crate) fn pool_status(&self) -> RequestLogPoolStatus {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.pool_status(),
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.pool_status(),
        }
    }

    /// Inserts one terminal event without changing schema-owned defaults.
    pub async fn insert(
        &self,
        event: &RequestLogEvent,
    ) -> Result<RequestLogInsertOutcome, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.insert(event).await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.insert(event).await,
        }
    }

    /// Persists financial facts before attempting the independent log projection.
    pub async fn insert_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<RequestLogBatchInsertResult>, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.insert_batch(events).await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.insert_batch(events).await,
        }
    }

    /// Projects events whose independent financial facts are already durable.
    pub(crate) async fn project_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<RequestLogBatchInsertResult>, RepositoryError> {
        match &self.backend {
            LogsBackend::Postgres(repository) => repository.project_batch(events).await,
            #[cfg(feature = "sqlite-backend")]
            LogsBackend::Sqlite { repository, .. } => repository.project_batch(events).await,
        }
    }
}

#[derive(Clone)]
enum MeteringBackend {
    Postgres(PostgresMeteringRepository),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteMeteringRepository),
}

/// Immutable financial facts, independent of the request-log projection.
#[derive(Clone)]
pub struct MeteringRepository {
    backend: MeteringBackend,
}

impl MeteringRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: MeteringBackend::Postgres(PostgresMeteringRepository::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: MeteringBackend::Sqlite(SqliteMeteringRepository::new(database)),
        }
    }

    fn from_postgres(repository: PostgresMeteringRepository) -> Self {
        Self {
            backend: MeteringBackend::Postgres(repository),
        }
    }

    pub async fn reconciliation_counts(
        &self,
    ) -> Result<MeteringReconciliationCounts, RepositoryError> {
        match &self.backend {
            MeteringBackend::Postgres(repository) => repository.reconciliation_counts().await,
            #[cfg(feature = "sqlite-backend")]
            MeteringBackend::Sqlite(repository) => repository.reconciliation_counts().await,
        }
    }

    pub async fn record_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
        match &self.backend {
            MeteringBackend::Postgres(repository) => repository.record_batch(events).await,
            #[cfg(feature = "sqlite-backend")]
            MeteringBackend::Sqlite(repository) => repository.record_batch(events).await,
        }
    }

    pub(crate) async fn load_pending(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogIngestRecord>, RepositoryError> {
        match &self.backend {
            MeteringBackend::Postgres(repository) => repository.load_pending(limit).await,
            #[cfg(feature = "sqlite-backend")]
            MeteringBackend::Sqlite(repository) => repository.load_pending(limit).await,
        }
    }

    pub(crate) async fn materialize(
        &self,
        receipts: &[IngestReceipt],
        events: &[RequestLogEvent],
    ) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
        match &self.backend {
            MeteringBackend::Postgres(repository) => repository.materialize(receipts, events).await,
            #[cfg(feature = "sqlite-backend")]
            MeteringBackend::Sqlite(repository) => repository.materialize(receipts, events).await,
        }
    }

    pub(crate) async fn defer(
        &self,
        receipts: &[IngestReceipt],
        code: &str,
        retry_seconds: i64,
    ) -> Result<(), RepositoryError> {
        match &self.backend {
            MeteringBackend::Postgres(repository) => {
                repository.defer(receipts, code, retry_seconds).await
            }
            #[cfg(feature = "sqlite-backend")]
            MeteringBackend::Sqlite(repository) => {
                repository.defer(receipts, code, retry_seconds).await
            }
        }
    }
}

#[derive(Clone)]
enum SettlementBackend {
    Postgres(PostgresSettlementRepository),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteSettlementRepository),
}

/// Single-transaction settlement claims against immutable financial facts.
#[derive(Clone)]
pub struct SettlementRepository {
    backend: SettlementBackend,
}

impl SettlementRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: SettlementBackend::Postgres(PostgresSettlementRepository::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: SettlementBackend::Sqlite(SqliteSettlementRepository::new(database)),
        }
    }

    fn from_postgres(repository: PostgresSettlementRepository) -> Self {
        Self {
            backend: SettlementBackend::Postgres(repository),
        }
    }

    /// Claims and applies one eligible financial fact in a single transaction.
    pub async fn settle(
        &self,
        request_log_id: Uuid,
    ) -> Result<RequestLogSettlementOutcome, RepositoryError> {
        match &self.backend {
            SettlementBackend::Postgres(repository) => repository.settle(request_log_id).await,
            #[cfg(feature = "sqlite-backend")]
            SettlementBackend::Sqlite(repository) => repository.settle(request_log_id).await,
        }
    }

    /// Claims and applies a set of billable terminal logs in one transaction.
    pub async fn settle_batch(
        &self,
        request_log_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, RequestLogSettlementOutcome)>, RepositoryError> {
        match &self.backend {
            SettlementBackend::Postgres(repository) => {
                repository.settle_batch(request_log_ids).await
            }
            #[cfg(feature = "sqlite-backend")]
            SettlementBackend::Sqlite(repository) => repository.settle_batch(request_log_ids).await,
        }
    }

    /// Reconciles outstanding settlement work after a restart.
    pub async fn settle_pending(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogSettlementOutcome>, RepositoryError> {
        match &self.backend {
            SettlementBackend::Postgres(repository) => repository.settle_pending(limit).await,
            #[cfg(feature = "sqlite-backend")]
            SettlementBackend::Sqlite(repository) => repository.settle_pending(limit).await,
        }
    }

    pub(crate) async fn settlement_backlog(
        &self,
    ) -> Result<RequestLogSettlementBacklog, RepositoryError> {
        match &self.backend {
            SettlementBackend::Postgres(repository) => repository.settlement_backlog().await,
            #[cfg(feature = "sqlite-backend")]
            SettlementBackend::Sqlite(repository) => repository.settlement_backlog().await,
        }
    }
}
