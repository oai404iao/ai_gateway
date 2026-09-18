//! Experimental SQLite storage primitives, not a selectable server backend.
//! SQL access stays in persistence; application code must use operation-specific repositories.

mod decimal;

pub use decimal::SqliteDecimal;

use std::{path::Path, time::Duration};

use sqlx::{
    Sqlite, SqliteConnection, SqlitePool, Transaction,
    pool::PoolConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Development-only file database. The caller must own a private local directory.
/// Process ownership, schema identity and migrations are not yet implemented.
pub struct SqliteDatabase {
    writer: SqlitePool,
    readers: SqlitePool,
}

impl SqliteDatabase {
    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(sqlx::Error::Configuration(
                "SQLite requires an absolute file path".into(),
            ));
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT)
            .pragma("recursive_triggers", "ON")
            .pragma("read_uncommitted", "OFF");
        let writer = pool_options(1, false).connect_with(options.clone()).await?;
        let readers = pool_options(4, true)
            .connect_with(
                options
                    .create_if_missing(false)
                    .read_only(true)
                    .pragma("query_only", "ON"),
            )
            .await;
        match readers {
            Ok(readers) => Ok(Self { writer, readers }),
            Err(error) => {
                writer.close().await;
                Err(error)
            }
        }
    }

    pub async fn acquire_read(&self) -> Result<PoolConnection<Sqlite>, sqlx::Error> {
        self.readers.acquire().await
    }

    /// Takes the sole write connection and acquires the SQLite write lock before reading.
    /// Dropping an uncommitted transaction schedules rollback; cancelling COMMIT has an
    /// uncertain outcome. Callers must not issue BEGIN/COMMIT SQL.
    pub async fn begin_write(&self) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
        self.writer.begin_with("BEGIN IMMEDIATE").await
    }

    pub async fn close(&self) {
        self.readers.close().await;
        self.writer.close().await;
    }
}

fn pool_options(max_connections: u32, read_only: bool) -> SqlitePoolOptions {
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .after_connect(move |connection, _| {
            Box::pin(async move { verify_connection(connection, read_only).await })
        })
}

async fn verify_connection(
    connection: &mut SqliteConnection,
    read_only: bool,
) -> Result<(), sqlx::Error> {
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut *connection)
        .await?;
    if mode != "wal" {
        return Err(sqlx::Error::Protocol("SQLite WAL mode is required".into()));
    }
    for (query, expected) in [
        ("PRAGMA synchronous", 2_i64),
        ("PRAGMA foreign_keys", 1),
        ("PRAGMA recursive_triggers", 1),
        ("PRAGMA read_uncommitted", 0),
        ("PRAGMA busy_timeout", BUSY_TIMEOUT.as_millis() as i64),
        ("PRAGMA query_only", i64::from(read_only)),
    ] {
        let actual: i64 = sqlx::query_scalar(query)
            .fetch_one(&mut *connection)
            .await?;
        if actual != expected {
            return Err(sqlx::Error::Protocol(
                "SQLite connection policy verification failed".into(),
            ));
        }
    }
    Ok(())
}
