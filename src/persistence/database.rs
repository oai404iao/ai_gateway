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
/// feature-gated SQLite discriminator accompanies its schema/type foundation
/// but is not returned by `DatabaseConnectOptions` until dispatch is added.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseBackend {
    Postgres,
    #[cfg(feature = "sqlite-backend")]
    Sqlite,
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

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.postgres.commit().await
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.postgres.rollback().await
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
    use super::{DatabaseBackend, DatabaseConnectOptions};

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
}
