//! PostgreSQL migration execution and release-wide transaction boundaries.

use std::collections::{HashMap, HashSet};

use sqlx::{
    Connection, PgConnection, PgPool, Postgres, Transaction,
    migrate::{AppliedMigration, Migrate, MigrateError},
};
use thiserror::Error;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

// PostgreSQL cannot use a newly added enum value until its transaction commits.
const COMMIT_BARRIERS: &[i64] = &[34, 46];

#[derive(Debug, Error)]
pub enum MigrationRunError {
    #[error(transparent)]
    CapabilityCutover(#[from] super::capability_cutover::io::CapabilityCutoverIoError),
    #[error(
        "channel {channel_id} has invalid legacy authentication or credential scope; repair or delete it before upgrading"
    )]
    LegacyCredential { channel_id: uuid::Uuid },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] MigrateError),
    #[error(
        "migration {version} cannot run because atomic migration batches do not permit no-transaction migrations"
    )]
    NonTransactional { version: i64 },
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrationRunError> {
    let mut connection = pool.acquire().await?;
    connection.lock().await?;
    let result = run_locked_migrations(&mut connection).await;
    let unlock = connection.unlock().await;
    match result {
        Err(error) => {
            if let Err(unlock_error) = unlock {
                tracing::error!(
                    %unlock_error,
                    "database migration advisory lock could not be released after rollback"
                );
            }
            Err(error)
        }
        Ok(()) => {
            unlock?;
            Ok(())
        }
    }
}

async fn run_locked_migrations(connection: &mut PgConnection) -> Result<(), MigrationRunError> {
    loop {
        let mut transaction = connection.begin().await?;
        let result = apply_next_migration_batch(&mut transaction).await;
        match result {
            Ok(has_more) => {
                transaction.commit().await?;
                if !has_more {
                    return Ok(());
                }
            }
            Err(error) => {
                transaction.rollback().await?;
                return Err(error);
            }
        }
    }
}

async fn apply_next_migration_batch(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<bool, MigrationRunError> {
    (**transaction)
        .ensure_migrations_table("_sqlx_migrations")
        .await?;
    if let Some(version) = (**transaction).dirty_version("_sqlx_migrations").await? {
        return Err(MigrateError::Dirty(version).into());
    }

    let applied = (**transaction)
        .list_applied_migrations("_sqlx_migrations")
        .await?;
    validate_applied_migrations(&applied)?;
    let applied = applied
        .into_iter()
        .map(|migration| (migration.version, migration))
        .collect::<HashMap<_, _>>();
    let pending = MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .filter_map(|migration| match applied.get(&migration.version) {
            Some(applied) if migration.checksum != applied.checksum => {
                Some(Err(MigrateError::VersionMismatch(migration.version).into()))
            }
            Some(_) => None,
            None if migration.no_tx => Some(Err(MigrationRunError::NonTransactional {
                version: migration.version,
            })),
            None => Some(Ok(migration)),
        })
        .collect::<Result<Vec<_>, MigrationRunError>>()?;

    let batch_len = pending
        .iter()
        .position(|migration| COMMIT_BARRIERS.contains(&migration.version))
        .map_or(pending.len(), |index| index + 1);
    let has_more = batch_len < pending.len();
    for migration in pending.into_iter().take(batch_len) {
        if migration.version == 64 {
            let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, String, Option<String>, Option<String>)>(
                "SELECT c.id,c.name,c.base_url,c.upstream_auth_kind,c.upstream_auth_header_name,c.upstream_api_key
                 FROM channels c JOIN channel_groups g ON g.id=c.channel_group_id
                 WHERE c.deleted_at IS NULL AND g.connector_kind='openai_compatible' ORDER BY c.id")
                .fetch_all(&mut **transaction).await?;
            for (channel_id, name, target, kind, header, secret) in rows {
                if !super::upstream_credentials::validate_legacy_auth(
                    &name,
                    &target,
                    &kind,
                    header.as_deref(),
                    secret.as_deref(),
                ) {
                    return Err(MigrationRunError::LegacyCredential { channel_id });
                }
            }
        }
        if migration.version == 66 {
            super::capability_cutover::operation_split::storage::pg_prepare(transaction).await?;
        }
        (**transaction).apply("_sqlx_migrations", migration).await?;
        if migration.version == 65 {
            super::capability_cutover::activation::postgres(transaction).await?;
        }
        if migration.version == 66 {
            super::capability_cutover::operation_split::storage::pg_validate(transaction).await?;
        }
    }
    Ok(has_more)
}

fn validate_applied_migrations(applied: &[AppliedMigration]) -> Result<(), MigrationRunError> {
    let known = MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect::<HashSet<_>>();
    if let Some(migration) = applied
        .iter()
        .find(|migration| !known.contains(&migration.version))
    {
        return Err(MigrateError::VersionMissing(migration.version).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{COMMIT_BARRIERS, MIGRATOR};

    #[test]
    fn commit_barriers_match_historical_enum_additions() {
        let enum_additions = MIGRATOR
            .iter()
            .filter(|migration| {
                let sql = migration.sql.as_str().to_ascii_lowercase();
                sql.contains("alter type") && sql.contains("add value")
            })
            .map(|migration| migration.version)
            .collect::<Vec<_>>();

        assert_eq!(enum_additions, COMMIT_BARRIERS);
    }

    #[test]
    fn embedded_migrations_never_opt_out_of_transactions() {
        assert!(MIGRATOR.iter().all(|migration| !migration.no_tx));
    }
}
