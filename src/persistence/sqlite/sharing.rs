//! Sharing ledger identity and membership reads under the database's process lease.

use super::{
    SqliteControlPlaneRepository, SqliteDatabase, SqliteUuid, control_plane::SharingGroupAuditRow,
};
use crate::{domain::codex_sharing::SharingGroup, persistence::RepositoryError};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct SqliteSharingLease {
    database: Arc<SqliteDatabase>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
    _owner: Arc<super::ownership::DatabaseOwner>,
}
impl SqliteSharingLease {
    pub(crate) async fn ping(&self) -> Result<(), RepositoryError> {
        // The ledger is checked at claim and never changed by cooperating writers.
        // Verify the retained file lease without mistaking pool contention for ownership loss.
        self.database
            .pools()
            .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;
        Ok(())
    }
}
impl SqliteControlPlaneRepository {
    pub(crate) async fn claim_sharing_ledger(
        &self,
        ledger: Uuid,
    ) -> Result<SqliteSharingLease, RepositoryError> {
        let owner = Arc::clone(
            &self
                .database
                .pools()
                .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?
                .owner,
        );
        let guard = Arc::clone(&self.database.sharing_owner)
            .try_lock_owned()
            .map_err(|_| RepositoryError::Conflict)?;
        let mut tx = self
            .database
            .begin_write()
            .await
            .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;
        sqlx::query("INSERT INTO codex_sharing_ledger(singleton,ledger_id) VALUES (1,?) ON CONFLICT DO NOTHING")
            .bind(SqliteUuid(ledger)).execute(&mut *tx).await?;
        let id = sqlx::query_scalar::<_, SqliteUuid>("SELECT ledger_id FROM codex_sharing_ledger")
            .fetch_one(&mut *tx)
            .await?
            .0;
        if id != ledger {
            return Err(RepositoryError::Conflict);
        }
        tx.commit().await?;
        Ok(SqliteSharingLease {
            database: Arc::clone(&self.database),
            _guard: guard,
            _owner: owner,
        })
    }
    pub async fn sharing_groups(
        &self,
        user: Option<Uuid>,
    ) -> Result<Vec<SharingGroup>, RepositoryError> {
        let mut reader = self
            .database
            .acquire_read()
            .await
            .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;
        let rows=sqlx::query_as::<_,SharingGroupAuditRow>(
            "SELECT * FROM codex_sharing_groups s WHERE ?1 IS NULL OR (
             EXISTS(SELECT 1 FROM json_each(s.seats) WHERE value=?1) AND
             EXISTS(SELECT 1 FROM users WHERE id=?1 AND status='active' AND deleted_at IS NULL AND NOT is_system)) ORDER BY s.id")
            .bind(user.map(SqliteUuid)).fetch_all(&mut *reader).await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_value(row.into_value()?).map_err(|_| RepositoryError::Validation)
            })
            .collect()
    }
}
