//! SQLite database identity and all-pending-migrations transaction protocol.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};
use uuid::Uuid;

use super::SqliteOpenError;

const APPLICATION_ID: i64 = 0x41494757;
const IDENTITY_VERSION: i64 = 1;
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

/// An ordered SQLite-only migration. Version numbers start at one, without gaps.
/// SQL is trusted, reviewed application code, not user input.
pub struct SqliteMigration<'a> {
    pub version: i64,
    pub description: &'a str,
    pub sql: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum SqliteMigrationError {
    #[error(transparent)]
    CapabilityCutover(#[from] crate::persistence::capability_cutover::io::CapabilityCutoverIoError),
    #[error(
        "channel {channel_id} has invalid legacy authentication or credential scope; repair or delete it before upgrading"
    )]
    LegacyCredential { channel_id: Uuid },
    #[error("SQLite migration manifest is invalid or requests nontransactional execution")]
    InvalidManifest,
    #[error("SQLite migration history differs from this binary")]
    HistoryMismatch,
    #[error("SQLite storage is unavailable")]
    Open(#[from] SqliteOpenError),
    #[error("SQLite migration transaction failed")]
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

fn validate_manifest(migrations: &[SqliteMigration<'_>]) -> Result<(), SqliteMigrationError> {
    for (index, migration) in migrations.iter().enumerate() {
        if migration.version != index as i64 + 1
            || migration.description.trim().is_empty()
            || migration.sql.trim().is_empty()
            || migration
                .sql
                .lines()
                .any(|line| line.trim().starts_with("-- no-transaction"))
        {
            return Err(SqliteMigrationError::InvalidManifest);
        }
    }
    Ok(())
}

pub(super) async fn validate_history(
    connection: &mut SqliteConnection,
    migrations: &[SqliteMigration<'_>],
) -> Result<usize, SqliteMigrationError> {
    let history = sqlx::query(
        "SELECT version, description, checksum FROM _gateway_sqlite_migrations ORDER BY version",
    )
    .fetch_all(connection)
    .await?;
    if history.len() > migrations.len() {
        return Err(SqliteMigrationError::HistoryMismatch);
    }
    for (row, expected) in history.iter().zip(migrations) {
        if row.try_get::<i64, _>("version")? != expected.version
            || row.try_get::<String, _>("description")? != expected.description
            || row.try_get::<Vec<u8>, _>("checksum")?
                != Sha256::digest(expected.sql.as_bytes()).as_slice()
        {
            return Err(SqliteMigrationError::HistoryMismatch);
        }
    }
    Ok(history.len())
}

pub(super) async fn run(
    pool: &SqlitePool,
    database_id: Uuid,
    migrations: &[SqliteMigration<'_>],
) -> Result<usize, SqliteMigrationError> {
    validate_manifest(migrations)?;
    let mut connection = pool.acquire().await?;
    // A cancelled migration must not return a connection carrying this commit hook to the pool.
    connection.close_on_drop();
    let capability_cutover = migrations.iter().any(|migration| {
        migration.version == 5
            && migration.description == "canonical upstream operation capabilities"
    });
    let operation_split = migrations.iter().any(|migration| {
        migration.version == 6
            && migration.description == "six operation routing and connector names"
    });
    let credential_ownership = migrations.iter().any(|migration| {
        migration.version == 8 && migration.description == "channel owned credentials and sharing"
    });
    if capability_cutover || operation_split || credential_ownership {
        sqlx::query("PRAGMA foreign_keys=OFF")
            .execute(&mut *connection)
            .await?;
    }
    let allow_commit = Arc::new(AtomicBool::new(false));
    let commit_gate = Arc::clone(&allow_commit);
    connection
        .lock_handle()
        .await?
        .set_commit_hook(move || commit_gate.load(Ordering::Acquire));
    let rollback_observed = Arc::new(AtomicBool::new(false));
    let rollback_flag = Arc::clone(&rollback_observed);
    connection
        .lock_handle()
        .await?
        .set_rollback_hook(move || rollback_flag.store(true, Ordering::Release));
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
    super::functions::set_transaction_time(&mut transaction).await?;
    if check_identity(&mut transaction).await? != Some(database_id) {
        return Err(SqliteOpenError::ForeignDatabase.into());
    }
    let applied = validate_history(&mut transaction, migrations).await?;
    for migration in &migrations[applied..] {
        if migration.version == 4
            && migration.description == "independent upstream credential identities"
        {
            let rows = sqlx::query_as::<_, (super::SqliteUuid, String, String, String, Option<String>, Option<String>)>(
                "SELECT c.id,c.name,c.base_url,c.upstream_auth_kind,c.upstream_auth_header_name,c.upstream_api_key
                 FROM channels c JOIN channel_groups g ON g.id=c.channel_group_id
                 WHERE c.deleted_at IS NULL AND g.connector_kind='openai_compatible' ORDER BY c.id")
                .fetch_all(&mut *transaction).await?;
            for (channel_id, name, target, kind, header, secret) in rows {
                if !crate::persistence::upstream_credentials::validate_legacy_auth(
                    &name,
                    &target,
                    &kind,
                    header.as_deref(),
                    secret.as_deref(),
                ) {
                    return Err(SqliteMigrationError::LegacyCredential {
                        channel_id: channel_id.0,
                    });
                }
            }
        }
        if credential_ownership && migration.version == 8 {
            crate::persistence::capability_cutover::credential_ownership::sqlite_prepare(
                &mut transaction,
            )
            .await?;
        }
        sqlx::Executor::execute(
            &mut *transaction,
            sqlx::AssertSqlSafe(migration.sql.to_owned()),
        )
        .await?;
        if capability_cutover && migration.version == 5 {
            crate::persistence::capability_cutover::activation::sqlite(&mut transaction).await?;
        }
        if operation_split && migration.version == 6 {
            crate::persistence::capability_cutover::operation_split::storage::sqlite_apply(
                &mut transaction,
            )
            .await?;
        }
        if credential_ownership && migration.version == 8 {
            crate::persistence::upstream_topology::sqlite_load_control_plane(&mut transaction)
                .await
                .map_err(
                    crate::persistence::capability_cutover::io::CapabilityCutoverIoError::from,
                )?;
        }
        if rollback_observed.load(Ordering::Acquire) {
            return Err(SqliteMigrationError::InvalidManifest);
        }
        sqlx::query(
            "INSERT INTO _gateway_sqlite_migrations (version, description, checksum) VALUES (?, ?, ?)",
        )
        .bind(migration.version)
        .bind(migration.description)
        .bind(Sha256::digest(migration.sql.as_bytes()).to_vec())
        .execute(&mut *transaction)
        .await?;
    }
    if validate_history(&mut transaction, migrations).await? != migrations.len() {
        return Err(SqliteMigrationError::HistoryMismatch);
    }
    if check_identity(&mut transaction).await? != Some(database_id) {
        return Err(SqliteOpenError::ForeignDatabase.into());
    }
    if (capability_cutover || operation_split || credential_ownership)
        && sqlx::query("PRAGMA foreign_key_check")
            .fetch_optional(&mut *transaction)
            .await?
            .is_some()
    {
        return Err(SqliteMigrationError::HistoryMismatch);
    }
    allow_commit.store(true, Ordering::Release);
    transaction.commit().await?;
    connection.close().await?;
    Ok(migrations.len() - applied)
}
