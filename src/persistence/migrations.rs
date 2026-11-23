//! Initialize a fresh PostgreSQL database or verify its installed baseline.

use sqlx::{PgPool, migrate::Migrate};
use thiserror::Error;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

const BASELINE_LOCK: i64 = 0x4149_4757_4241_5345;
const HISTORY: &str = "public._sqlx_migrations";

#[derive(Debug, Error)]
pub enum MigrationRunError {
    #[error("incompatible PostgreSQL database; initialize the gateway with a fresh database")]
    IncompatibleDatabase,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrationRunError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_catalog.pg_advisory_xact_lock($1)")
        .bind(BASELINE_LOCK)
        .execute(&mut *tx)
        .await?;

    let baseline = MIGRATOR
        .iter()
        .next()
        .expect("embedded PostgreSQL baseline");
    let has_history: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_class
         WHERE relnamespace='public'::regnamespace AND relname='_sqlx_migrations' AND relkind='r')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if has_history {
        let applied = tx.list_applied_migrations(HISTORY).await?;
        if !matches!(&applied[..], [row] if row.version == baseline.version && row.checksum == baseline.checksum)
            || tx.dirty_version(HISTORY).await?.is_some()
        {
            return Err(MigrationRunError::IncompatibleDatabase);
        }
    } else {
        // Compose may install pg_stat_statements before the gateway starts. Exempt
        // the extension and its actual members, not objects that share their names.
        let empty: bool = sqlx::query_scalar(
            "WITH observability AS (SELECT oid FROM pg_catalog.pg_extension WHERE extname='pg_stat_statements'),
             extension_objects AS (
                SELECT 'pg_catalog.pg_extension'::regclass AS classid, oid AS objid FROM observability
                UNION ALL SELECT classid, objid FROM pg_catalog.pg_depend
                  WHERE refclassid='pg_catalog.pg_extension'::regclass AND deptype='e'
                    AND refobjid IN (SELECT oid FROM observability))
             SELECT NOT EXISTS(SELECT 1 FROM pg_catalog.pg_depend
                WHERE refclassid='pg_catalog.pg_namespace'::regclass AND refobjid='public'::regnamespace
                  AND (classid,objid) NOT IN (SELECT classid,objid FROM extension_objects))
             AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_namespace
                WHERE nspname !~ '^pg_' AND nspname NOT IN ('public','information_schema'))
             AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_largeobject_metadata)",
        )
        .fetch_one(&mut *tx)
        .await?;
        if !empty {
            return Err(MigrationRunError::IncompatibleDatabase);
        }
        tx.ensure_migrations_table(HISTORY).await?;
        tx.apply(HISTORY, baseline).await?;
    }
    tx.commit().await?;
    Ok(())
}
