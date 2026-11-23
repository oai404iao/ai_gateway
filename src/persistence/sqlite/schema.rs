//! Managed database identity and atomic installation of the embedded business schema.

use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use uuid::Uuid;

use super::{SqliteDatabase, SqliteOpenError};

const BASELINE: &str = include_str!("../../../migrations/sqlite/0001_baseline.sql");
const BASELINE_DESCRIPTION: &str = "gateway baseline";

const APPLICATION_ID: i64 = 0x41494757;
const IDENTITY_VERSION: i64 = 2;
const IDENTITY_DDL: &str = "CREATE TABLE _gateway_sqlite_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    database_id TEXT NOT NULL
) STRICT";
const HISTORY_DDL: &str = "CREATE TABLE _gateway_sqlite_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    description TEXT NOT NULL,
    checksum BLOB NOT NULL CHECK (length(checksum) = 32)
) STRICT";
const IDENTITY_GUARDS: [(&str, &str); 2] = [
    (
        "gateway_identity_no_update",
        "CREATE TRIGGER gateway_identity_no_update BEFORE UPDATE ON _gateway_sqlite_identity
         BEGIN SELECT RAISE(ABORT, 'gateway_identity_immutable'); END",
    ),
    (
        "gateway_identity_no_delete",
        "CREATE TRIGGER gateway_identity_no_delete BEFORE DELETE ON _gateway_sqlite_identity
         BEGIN SELECT RAISE(ABORT, 'gateway_identity_immutable'); END",
    ),
];

#[derive(Debug, thiserror::Error)]
pub enum SqliteSchemaError {
    #[error("SQLite database does not match the current baseline; use a fresh database")]
    HistoryMismatch,
    #[error("SQLite database contains a foreign key violation")]
    ForeignKeyViolation,
    #[error("SQLite storage is unavailable")]
    Open(#[from] SqliteOpenError),
    #[error("SQLite baseline installation failed")]
    Storage(#[from] sqlx::Error),
}

pub(super) async fn check_identity(
    connection: &mut SqliteConnection,
) -> Result<Option<Uuid>, SqliteOpenError> {
    let application_id: i64 = sqlx::query_scalar("PRAGMA application_id")
        .fetch_one(&mut *connection)
        .await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *connection)
        .await?;
    if application_id == 0 && version == 0 {
        let objects: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_schema")
            .fetch_one(&mut *connection)
            .await?;
        return if objects == 0 {
            Ok(None)
        } else {
            Err(SqliteOpenError::ForeignDatabase)
        };
    }
    if application_id != APPLICATION_ID || version != IDENTITY_VERSION {
        return Err(SqliteOpenError::ForeignDatabase);
    }
    for (table, ddl) in [
        ("_gateway_sqlite_identity", IDENTITY_DDL),
        ("_gateway_sqlite_migrations", HISTORY_DDL),
    ] {
        let actual: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?")
                .bind(table)
                .fetch_optional(&mut *connection)
                .await?;
        if actual.as_deref() != Some(ddl) {
            return Err(SqliteOpenError::ForeignDatabase);
        }
    }
    for (trigger, ddl) in IDENTITY_GUARDS {
        let actual: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?")
                .bind(trigger)
                .fetch_optional(&mut *connection)
                .await?;
        if actual.as_deref() != Some(ddl) {
            return Err(SqliteOpenError::ForeignDatabase);
        }
    }
    let identities: Vec<(i64, String)> =
        sqlx::query_as("SELECT singleton, database_id FROM _gateway_sqlite_identity")
            .fetch_all(&mut *connection)
            .await?;
    let [(1, database_id)] = identities.as_slice() else {
        return Err(SqliteOpenError::ForeignDatabase);
    };
    let parsed = Uuid::parse_str(database_id).map_err(|_| SqliteOpenError::ForeignDatabase)?;
    if parsed.to_string() != *database_id || parsed.is_nil() {
        return Err(SqliteOpenError::ForeignDatabase);
    }
    Ok(Some(parsed))
}

pub(super) async fn initialize(pool: &SqlitePool, identity: Uuid) -> Result<Uuid, SqliteOpenError> {
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(actual) = check_identity(&mut transaction).await? {
        if identity != actual {
            return Err(SqliteOpenError::ForeignDatabase);
        }
        transaction.rollback().await?;
        return Ok(identity);
    }
    sqlx::query(IDENTITY_DDL).execute(&mut *transaction).await?;
    sqlx::query(HISTORY_DDL).execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO _gateway_sqlite_identity VALUES (1, ?)")
        .bind(identity.to_string())
        .execute(&mut *transaction)
        .await?;
    for (_, ddl) in IDENTITY_GUARDS {
        sqlx::Executor::execute(&mut *transaction, ddl).await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA application_id = {APPLICATION_ID}"
    )))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA user_version = {IDENTITY_VERSION}"
    )))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(identity)
}

pub(super) async fn baseline_installed(
    connection: &mut SqliteConnection,
) -> Result<bool, SqliteSchemaError> {
    let history: Vec<(i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT version,description,checksum FROM _gateway_sqlite_migrations LIMIT 2",
    )
    .fetch_all(&mut *connection)
    .await?;
    match history.as_slice() {
        [(1, description, checksum)]
            if description == BASELINE_DESCRIPTION
                && checksum.as_slice() == Sha256::digest(BASELINE.as_bytes()).as_slice() =>
        {
            Ok(true)
        }
        [] => {
            // Missing history is new only when identity initialization is the sole prior write.
            let nonempty: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT IN (
                    '_gateway_sqlite_identity','_gateway_sqlite_migrations',
                    'gateway_identity_no_delete','gateway_identity_no_update'))",
            )
            .fetch_one(connection)
            .await?;
            if nonempty {
                Err(SqliteSchemaError::HistoryMismatch)
            } else {
                Ok(false)
            }
        }
        _ => Err(SqliteSchemaError::HistoryMismatch),
    }
}

impl SqliteDatabase {
    /// Installs the embedded baseline once. Returns 1 for a fresh database, 0 on reopen.
    pub async fn install_schema(&self) -> Result<usize, SqliteSchemaError> {
        let pools = self.pools()?;
        let mut writer = pools.writer.acquire().await?;
        // SQLite may roll back a failed COMMIT itself. Discard the install connection
        // instead of returning a stale SQLx transaction depth to the serving pool.
        writer.close_on_drop();
        let mut tx = writer.begin_with("BEGIN IMMEDIATE").await?;
        super::functions::set_transaction_time(&mut tx).await?;
        pools.owner.verify()?;
        if check_identity(&mut tx).await? != Some(self.database_id()) {
            return Err(SqliteOpenError::ForeignDatabase.into());
        }
        let installed = baseline_installed(&mut tx).await?;
        if !installed {
            sqlx::raw_sql(BASELINE).execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO _gateway_sqlite_migrations (version, description, checksum) VALUES (1, ?, ?)",
            )
            .bind(BASELINE_DESCRIPTION)
            .bind(Sha256::digest(BASELINE.as_bytes()).to_vec())
            .execute(&mut *tx)
            .await?;
        }
        if sqlx::query("PRAGMA foreign_key_check")
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        {
            return Err(SqliteSchemaError::ForeignKeyViolation);
        }
        tx.commit().await?;
        Ok(usize::from(!installed))
    }
}
