//! Database-neutral persistence boundary and backend implementations.

mod database;
mod postgres;
#[cfg(feature = "sqlite-backend")]
mod sqlite;

pub use database::{
    DatabaseBackend, DatabaseConnectOptions, DatabasePool, MIGRATOR, POSTGRES_MIGRATOR,
    RepositoryTransaction, run_migrations,
};
pub use postgres::*;
#[cfg(feature = "sqlite-backend")]
pub use sqlite::{SQLITE_MIGRATOR, SqliteDecimal, SqliteStringList, SqliteUuidList};
