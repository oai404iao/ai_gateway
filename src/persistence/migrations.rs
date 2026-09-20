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
    let connection = &mut **transaction;
    connection
        .ensure_migrations_table("_sqlx_migrations")
        .await?;
    if let Some(version) = connection.dirty_version("_sqlx_migrations").await? {
        return Err(MigrateError::Dirty(version).into());
    }

    let applied = connection
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
        connection.apply("_sqlx_migrations", migration).await?;
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
