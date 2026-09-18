//! Experimental SQLite storage primitives, not a selectable server backend.
//! SQL access stays in persistence; application code must use operation-specific repositories.

mod decimal;
mod functions;
mod migrations;
mod ownership;
mod schema;
mod types;

pub use decimal::{
    SqliteAmount, SqliteDecimal, SqliteNumeric, SqliteSharingAmount, SqliteTokenRate,
    SqliteUnitPrice,
};
pub use migrations::{SqliteMigration, SqliteMigrationError};
pub use types::{SqliteDate, SqliteTimestamp, SqliteUuid};

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use sqlx::{
    Connection, Sqlite, SqliteConnection, SqlitePool, Transaction,
    pool::PoolConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use uuid::Uuid;

use ownership::DatabaseOwner;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum SqliteOpenError {
    #[error("SQLite requires a private owned directory and regular, unaliased 0600 files")]
    UnsafePath,
    #[error("SQLite directory or database is already owned by another opener")]
    AlreadyOwned,
    #[error("SQLite directory or database identity changed; reopen requires operator recovery")]
    IdentityChanged,
    #[error("SQLite database identity is absent, invalid or newer than this binary")]
    ForeignDatabase,
    #[error("SQLite process ownership currently requires Linux")]
    UnsupportedPlatform,
    #[error("SQLite directory filesystem is unsupported")]
    UnsupportedFilesystem,
    #[error("SQLite database is closed")]
    Closed,
    #[error("SQLite filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("SQLite connection failed")]
    Storage(#[from] sqlx::Error),
}

impl From<rustix::io::Errno> for SqliteOpenError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(error.into())
    }
}

/// Development-only file database. Call `install_schema` before accessing business tables.
/// Server repositories are not yet wired to this backend.
/// All openers must cooperate, and claimed paths must remain unchanged until process exit.
/// Closing pools allows same-process reuse; another process must wait for this process to exit.
pub struct SqliteDatabase {
    pools: Mutex<Option<Arc<DatabasePools>>>,
    database_id: Uuid,
    owner_closed: tokio::sync::watch::Receiver<()>,
}

struct DatabasePools {
    writer: SqlitePool,
    readers: SqlitePool,
    owner: Arc<DatabaseOwner>,
}

impl SqliteDatabase {
    pub async fn open(path: &Path) -> Result<Self, SqliteOpenError> {
        let owner = Arc::new(DatabaseOwner::acquire(path)?);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // Caller cancellation must not abandon initialization between opening the driver and
        // attaching its native lifetime lease. Runtime shutdown requires closing databases first.
        tokio::spawn(async move {
            let result = Self::open_owned(owner).await;
            if let Err(Ok(database)) = sender.send(result) {
                database.close().await;
            }
        });
        receiver.await.map_err(|_| SqliteOpenError::Closed)?
    }

    async fn open_owned(owner: Arc<DatabaseOwner>) -> Result<Self, SqliteOpenError> {
        let connection_owner = Arc::clone(&owner);
        let options = SqliteConnectOptions::new()
            .filename(owner.path())
            .create_if_missing(false)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT)
            .pragma("recursive_triggers", "ON")
            .pragma("read_uncommitted", "OFF")
            .collation("ag_api_format", |left, right| {
                let rank = |value: &str| {
                    crate::domain::ApiFormat::ALL
                        .iter()
                        .position(|format| format.as_str() == value)
                        .unwrap_or(usize::MAX)
                };
                (rank(left), left).cmp(&(rank(right), right))
            })
            // Native callbacks retain the logical opener through handle teardown; the separate
            // process lease also covers opening workers cancelled before callback registration.
            .collation("_gateway_owner", move |left, right| {
                let _lease = &connection_owner;
                left.cmp(right)
            });
        let mut probe = SqliteConnection::connect_with(&options.clone().read_only(true)).await?;
        functions::register(&mut probe).await?;
        let identity = migrations::check_identity(&mut probe).await;
        probe.close().await?;
        if identity?.is_some_and(|identity| identity != owner.database_id()) {
            return Err(SqliteOpenError::ForeignDatabase);
        }
        owner.verify()?;
        let options = options.journal_mode(SqliteJournalMode::Wal);
        let writer = pool_options(1, false, Arc::clone(&owner))
            .connect_with(options.clone())
            .await?;
        let database_id = match migrations::initialize(&writer, owner.database_id()).await {
            Ok(identity) => identity,
            Err(error) => {
                writer.close().await;
                return Err(error);
            }
        };
        let readers = pool_options(4, true, Arc::clone(&owner))
            .connect_with(
                options
                    .create_if_missing(false)
                    .read_only(true)
                    .pragma("query_only", "ON"),
            )
            .await;
        match readers {
            Ok(readers) => {
                owner.verify()?;
                let owner_closed = owner.closed();
                Ok(Self {
                    pools: Mutex::new(Some(Arc::new(DatabasePools {
                        writer,
                        readers,
                        owner,
                    }))),
                    database_id,
                    owner_closed,
                })
            }
            Err(error) => {
                writer.close().await;
                Err(error.into())
            }
        }
    }

    fn pools(&self) -> Result<Arc<DatabasePools>, SqliteOpenError> {
        let pools = self
            .pools
            .lock()
            .expect("SQLite pools lock poisoned")
            .clone()
            .ok_or(SqliteOpenError::Closed)?;
        pools.owner.verify()?;
        Ok(pools)
    }

    pub fn database_id(&self) -> Uuid {
        self.database_id
    }

    pub async fn acquire_read(&self) -> Result<PoolConnection<Sqlite>, SqliteOpenError> {
        let pools = self.pools()?;
        let mut connection = pools.readers.acquire().await?;
        functions::set_transaction_time(&mut connection).await?;
        pools.owner.verify()?;
        Ok(connection)
    }

    /// Takes the sole write connection and acquires the SQLite write lock before reading.
    /// Dropping an uncommitted transaction schedules rollback; cancelling COMMIT has an
    /// uncertain outcome. Callers must not issue BEGIN/COMMIT SQL.
    pub async fn begin_write(&self) -> Result<Transaction<'static, Sqlite>, SqliteOpenError> {
        let pools = self.pools()?;
        let mut transaction = pools.writer.begin_with("BEGIN IMMEDIATE").await?;
        functions::set_transaction_time(&mut transaction).await?;
        pools.owner.verify()?;
        Ok(transaction)
    }

    pub async fn migrate(
        &self,
        migrations: &[SqliteMigration<'_>],
    ) -> Result<usize, SqliteMigrationError> {
        let pools = self.pools()?;
        migrations::run(&pools.writer, self.database_id, migrations).await
    }

    pub async fn install_schema(&self) -> Result<usize, SqliteMigrationError> {
        self.migrate(schema::MIGRATIONS).await
    }

    pub async fn close(&self) {
        let pools = self
            .pools
            .lock()
            .expect("SQLite pools lock poisoned")
            .take();
        if let Some(pools) = pools {
            pools.readers.close().await;
            pools.writer.close().await;
        }
        // Pool close can precede callback/options teardown. Wait for the logical owner itself,
        // including when another caller started close or a previous close future was cancelled.
        let _ = self.owner_closed.clone().changed().await;
    }
}

fn pool_options(
    max_connections: u32,
    read_only: bool,
    owner: Arc<DatabaseOwner>,
) -> SqlitePoolOptions {
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .after_connect(move |connection, _| {
            let owner = Arc::clone(&owner);
            Box::pin(async move {
                functions::register(connection).await?;
                owner
                    .verify()
                    .map_err(|error| sqlx::Error::Configuration(Box::new(error)))?;
                verify_connection(connection, read_only).await
            })
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
