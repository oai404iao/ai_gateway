//! Database-neutral persistence boundary and backend implementations.

mod database;
mod error;
mod postgres;
mod records;
#[cfg(feature = "sqlite-backend")]
mod sqlite;

pub use database::{
    DatabaseBackend, DatabaseConnectOptions, DatabasePool, MIGRATOR, POSTGRES_MIGRATOR,
    RepositoryTransaction, run_migrations,
};
pub use error::RepositoryError;
pub use postgres::*;
pub use records::*;
#[cfg(feature = "sqlite-backend")]
pub use sqlite::{SQLITE_MIGRATOR, SqliteDecimal, SqliteStringList, SqliteUuidList};
