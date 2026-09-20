//! Read-only database pool observation for current-instance metrics.
//!
//! Both backends expose live pool counts without acquiring a connection, so a
//! metrics sample cannot itself change the load it reports.

use sqlx::PgPool;

#[cfg(feature = "sqlite-backend")]
use std::sync::Arc;

#[cfg(feature = "sqlite-backend")]
use super::sqlite::SqliteDatabase;

#[derive(Clone)]
enum Backend {
    Postgres(PgPool),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(Arc<SqliteDatabase>),
}

#[derive(Clone)]
pub struct DatabaseHealth {
    backend: Backend,
}

impl From<PgPool> for DatabaseHealth {
    fn from(pool: PgPool) -> Self {
        Self {
            backend: Backend::Postgres(pool),
        }
    }
}

impl DatabaseHealth {
    /// Builds development health observation over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: Backend::Sqlite(database),
        }
    }

    #[must_use]
    pub fn size(&self) -> u32 {
        match &self.backend {
            Backend::Postgres(pool) => pool.size(),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(database) => database.connection_counts().map_or(0, |counts| counts.0),
        }
    }

    #[must_use]
    pub fn idle(&self) -> usize {
        match &self.backend {
            Backend::Postgres(pool) => pool.num_idle(),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(database) => database.connection_counts().map_or(0, |counts| counts.1),
        }
    }
}
