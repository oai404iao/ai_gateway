//! Opaque database connection, pool, migration, and transaction boundary.

use std::{str::FromStr, time::Duration};

use sqlx::{
    PgPool, Postgres, Transaction,
    migrate::{MigrateError, Migrator},
    postgres::{PgConnectOptions, PgConnection, PgPoolOptions},
};

/// Database engines understood by the persistence boundary.
///
/// PostgreSQL remains the only connectable runtime repository backend. The
/// feature-gated SQLite discriminator accompanies its schema/type and
/// runtime-snapshot/Console auth-account foundation but is not returned by
/// `DatabaseConnectOptions` until complete dispatch is added.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseBackend {
    Postgres,
    #[cfg(feature = "sqlite-backend")]
    Sqlite,
}

/// Stable intent assigned to a repository transaction.
///
/// These intents let backend adapters choose appropriate transaction behavior
/// without exposing backend-specific isolation or tuning to application code.
/// M1 only freezes the contract; existing transactions retain their current
/// PostgreSQL or SQLite behavior until backend dispatch is implemented.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransactionIntent {
    /// Read a complete runtime or control-plane view from one consistent
    /// snapshot without admitting a partially published management change.
    ConsistentRead,
    /// Atomically apply a Console/control-plane mutation, its optimistic
    /// concurrency checks, and its audit record.
    ManagementWrite,
    /// Persist high-volume, idempotent request-log ingestion or projection
    /// work without imposing settlement semantics on that write path.
    RequestLogWrite,
    /// Claim request logs for settlement and atomically apply the resulting
    /// usage, billing, and account updates.
    Settlement,
}

impl TransactionIntent {
    /// Stable identifier used by persistence metrics and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConsistentRead => "consistent_read",
            Self::ManagementWrite => "management_write",
            Self::RequestLogWrite => "request_log_write",
            Self::Settlement => "settlement",
        }
    }
}

impl DatabaseBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite => "sqlite",
        }
    }
}

/// Parsed connection options without exposing a concrete SQLx backend to
/// process startup.
#[derive(Clone)]
pub struct DatabaseConnectOptions {
    postgres: PgConnectOptions,
}

impl DatabaseConnectOptions {
    #[must_use]
    pub const fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgres
    }

    #[must_use]
    pub fn password(mut self, password: &str) -> Self {
        self.postgres = self.postgres.password(password);
        self
    }

    pub async fn connect_pool(
        &self,
        max_connections: u32,
        acquire_timeout: Duration,
        application_name: &str,
    ) -> Result<DatabasePool, sqlx::Error> {
        PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(acquire_timeout)
            .connect_with(self.postgres.clone().application_name(application_name))
            .await
            .map(DatabasePool::from)
    }
}

impl FromStr for DatabaseConnectOptions {
    type Err = sqlx::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .parse::<PgConnectOptions>()
            .map(|postgres| Self { postgres })
    }
}

/// Cloneable pool handle exposed to repositories and pool-pressure metrics.
///
/// The concrete accessor is crate-private and should remain confined to the
/// backend implementation.
///
/// ```compile_fail
/// use ai_gateway::persistence::DatabasePool;
///
/// fn concrete_pool(pool: &DatabasePool) {
///     let _ = pool.postgres();
/// }
/// ```
#[derive(Clone)]
pub struct DatabasePool {
    postgres: PgPool,
}

impl DatabasePool {
    #[must_use]
    pub const fn backend(&self) -> DatabaseBackend {
        DatabaseBackend::Postgres
    }

    #[must_use]
    pub fn size(&self) -> u32 {
        self.postgres.size()
    }

    #[must_use]
    pub fn num_idle(&self) -> usize {
        self.postgres.num_idle()
    }

    pub(super) fn postgres(&self) -> &PgPool {
        &self.postgres
    }

    pub(super) async fn begin(&self) -> Result<RepositoryTransaction<'_>, sqlx::Error> {
        self.postgres.begin().await.map(RepositoryTransaction::new)
    }
}

impl From<PgPool> for DatabasePool {
    fn from(postgres: PgPool) -> Self {
        Self { postgres }
    }
}

/// Opaque repository transaction used across application/persistence
/// boundaries.
///
/// ```compile_fail
/// use ai_gateway::persistence::RepositoryTransaction;
///
/// fn concrete_transaction(transaction: &mut RepositoryTransaction<'_>) {
///     let _ = transaction.postgres();
/// }
/// ```
pub struct RepositoryTransaction<'connection> {
    postgres: Transaction<'connection, Postgres>,
}

impl<'connection> RepositoryTransaction<'connection> {
    fn new(postgres: Transaction<'connection, Postgres>) -> Self {
        Self { postgres }
    }

    pub(super) fn postgres(&mut self) -> &mut PgConnection {
        &mut self.postgres
    }

    pub async fn commit(self) -> Result<(), super::RepositoryError> {
        self.postgres.commit().await.map_err(Into::into)
    }

    pub async fn rollback(self) -> Result<(), super::RepositoryError> {
        self.postgres.rollback().await.map_err(Into::into)
    }
}

/// PostgreSQL migration history retained for integration and upgrade tests.
pub static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations");
pub use POSTGRES_MIGRATOR as MIGRATOR;

pub async fn run_migrations(pool: &DatabasePool) -> Result<(), MigrateError> {
    POSTGRES_MIGRATOR.run(pool.postgres()).await
}

#[cfg(test)]
mod tests {
    use super::{DatabaseBackend, DatabaseConnectOptions, TransactionIntent};

    #[test]
    fn postgres_options_expose_the_opaque_backend_discriminator() {
        let options = "postgres://user@127.0.0.1/database"
            .parse::<DatabaseConnectOptions>()
            .unwrap();

        assert_eq!(options.backend(), DatabaseBackend::Postgres);
        assert_eq!(options.backend().as_str(), "postgres");
    }

    #[cfg(feature = "sqlite-backend")]
    #[test]
    fn sqlite_backend_has_a_stable_discriminator() {
        assert_eq!(DatabaseBackend::Sqlite.as_str(), "sqlite");
    }

    #[test]
    fn invalid_database_url_is_rejected_before_pool_creation() {
        assert!(
            "not-a-database-url"
                .parse::<DatabaseConnectOptions>()
                .is_err()
        );
    }

    #[test]
    fn transaction_intent_identifiers_are_stable() {
        let intents = [
            TransactionIntent::ConsistentRead,
            TransactionIntent::ManagementWrite,
            TransactionIntent::RequestLogWrite,
            TransactionIntent::Settlement,
        ];

        assert_eq!(
            intents.map(TransactionIntent::as_str),
            [
                "consistent_read",
                "management_write",
                "request_log_write",
                "settlement",
            ]
        );
    }
}
