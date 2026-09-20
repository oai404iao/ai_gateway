//! Opaque provider operations and sharing ownership; callers cannot access a database transaction.

use super::codex_write::{PostgresCodexQuotaReset, PostgresCodexRefresh};
#[cfg(feature = "sqlite-backend")]
use super::sqlite::SqliteCodexOperation;
use super::*;
use chrono::{DateTime, Utc};
use sqlx::Connection;
use uuid::Uuid;

pub struct CodexRefresh<'a>(RefreshBackend<'a>);
enum RefreshBackend<'a> {
    Postgres(PostgresCodexRefresh<'a>),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteCodexOperation),
}
pub struct CodexQuotaReset<'a>(ResetBackend<'a>);
enum ResetBackend<'a> {
    Postgres(PostgresCodexQuotaReset<'a>),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteCodexOperation),
}
impl<'a> CodexRefresh<'a> {
    pub(super) fn postgres(guard: PostgresCodexRefresh<'a>) -> Self {
        Self(RefreshBackend::Postgres(guard))
    }
    #[cfg(feature = "sqlite-backend")]
    pub(super) fn sqlite(guard: SqliteCodexOperation) -> Self {
        Self(RefreshBackend::Sqlite(guard))
    }
    /// Persist dispatch intent after local validation and before any provider request.
    pub async fn prepare_dispatch(&mut self) -> Result<(), RepositoryError> {
        match &mut self.0 {
            RefreshBackend::Postgres(_) => Ok(()),
            #[cfg(feature = "sqlite-backend")]
            RefreshBackend::Sqlite(g) => g.prepare_dispatch().await,
        }
    }
    pub async fn unchanged(self) -> Result<(), RepositoryError> {
        match self.0 {
            RefreshBackend::Postgres(g) => g.unchanged().await,
            #[cfg(feature = "sqlite-backend")]
            RefreshBackend::Sqlite(g) => g.unchanged().await,
        }
    }
    pub async fn complete(self, update: CodexTokenRefreshUpdate) -> Result<(), RepositoryError> {
        match self.0 {
            RefreshBackend::Postgres(g) => g.complete(update).await,
            #[cfg(feature = "sqlite-backend")]
            RefreshBackend::Sqlite(g) => g.complete_refresh(update).await,
        }
    }
    pub async fn fail(
        self,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        match self.0 {
            RefreshBackend::Postgres(g) => g.fail(permanent, code, summary).await,
            #[cfg(feature = "sqlite-backend")]
            RefreshBackend::Sqlite(g) => g.fail(permanent, code, summary).await,
        }
    }
}
impl<'a> CodexQuotaReset<'a> {
    pub(super) fn postgres(guard: PostgresCodexQuotaReset<'a>) -> Self {
        Self(ResetBackend::Postgres(guard))
    }
    #[cfg(feature = "sqlite-backend")]
    pub(super) fn sqlite(guard: SqliteCodexOperation) -> Self {
        Self(ResetBackend::Sqlite(guard))
    }
    /// Persist dispatch intent after local validation and before any provider request.
    pub async fn prepare_dispatch(&mut self) -> Result<(), RepositoryError> {
        match &mut self.0 {
            ResetBackend::Postgres(_) => Ok(()),
            #[cfg(feature = "sqlite-backend")]
            ResetBackend::Sqlite(g) => g.prepare_dispatch().await,
        }
    }
    pub async fn complete(
        self,
        actor: Uuid,
        event: Uuid,
        requested: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows: i32,
    ) -> Result<Uuid, RepositoryError> {
        match self.0 {
            ResetBackend::Postgres(g) => {
                g.complete(actor, event, requested, outcome, windows).await
            }
            #[cfg(feature = "sqlite-backend")]
            ResetBackend::Sqlite(g) => {
                g.complete_reset(actor, event, requested, outcome, windows)
                    .await
            }
        }
    }
}

pub struct SharingLedgerLease(LedgerBackend);
enum LedgerBackend {
    Postgres(sqlx::PgConnection),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(super::sqlite::SqliteSharingLease),
}
impl SharingLedgerLease {
    pub(super) fn postgres(connection: sqlx::PgConnection) -> Self {
        Self(LedgerBackend::Postgres(connection))
    }
    #[cfg(feature = "sqlite-backend")]
    pub(super) fn sqlite(lease: super::sqlite::SqliteSharingLease) -> Self {
        Self(LedgerBackend::Sqlite(lease))
    }
    pub async fn ping(&mut self) -> Result<(), RepositoryError> {
        match &mut self.0 {
            LedgerBackend::Postgres(c) => Ok(c.ping().await?),
            #[cfg(feature = "sqlite-backend")]
            LedgerBackend::Sqlite(lease) => lease.ping().await,
        }
    }
    pub async fn close(self) -> Result<(), RepositoryError> {
        match self.0 {
            LedgerBackend::Postgres(c) => Ok(c.close().await?),
            #[cfg(feature = "sqlite-backend")]
            LedgerBackend::Sqlite(_) => Ok(()),
        }
    }
}
