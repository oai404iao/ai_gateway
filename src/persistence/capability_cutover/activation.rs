//! Atomic topology transfer and identity retargeting, called only by startup migration.

use sqlx::{Postgres, Transaction};

use super::{history, io};

pub async fn postgres(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), io::CapabilityCutoverIoError> {
    io::pg_transfer(transaction, chrono::Utc::now()).await?;
    history::pg_retarget_history(transaction).await?;
    sqlx::raw_sql(include_str!("codex-retirement-postgres.sql"))
        .execute(&mut **transaction)
        .await?;
    super::super::upstream_topology::pg_load_control_plane(transaction).await?;
    Ok(())
}

/// Requires the startup migrator's close-on-drop writer with foreign keys
/// disabled before BEGIN. Any failure must roll back schema and data together.
#[cfg(feature = "sqlite-backend")]
pub async fn sqlite(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), io::CapabilityCutoverIoError> {
    io::sqlite_transfer(transaction, chrono::Utc::now()).await?;
    sqlx::raw_sql(include_str!("codex-retirement-sqlite.sql"))
        .execute(&mut **transaction)
        .await?;
    history::sqlite_retarget_codex_identity(transaction).await?;
    history::sqlite_retarget_history(transaction).await?;
    sqlx::raw_sql(include_str!("sqlite-codex-guards.sql"))
        .execute(&mut **transaction)
        .await?;
    super::super::upstream_topology::sqlite_load_control_plane(transaction).await?;
    Ok(())
}
