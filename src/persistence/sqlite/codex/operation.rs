//! Durable pre-provider intent: no SQLite write transaction survives an external call.

use super::*;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

pub(crate) struct SqliteCodexOperation {
    database: Arc<super::super::SqliteDatabase>,
    channel: Uuid,
    attempt: Uuid,
    generation: i64,
    updated_at: DateTime<Utc>,
    credits: Option<i64>,
    started_at: Option<DateTime<Utc>>,
    kind: &'static str,
    _lock: OwnedMutexGuard<()>,
    _owner: Arc<super::super::ownership::DatabaseOwner>,
}

fn credential_lock(database: &super::super::SqliteDatabase, id: Uuid) -> Arc<Mutex<()>> {
    let mut locks = database
        .codex_operations
        .lock()
        .expect("Codex lock map poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&id).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(id, Arc::downgrade(&lock));
    lock
}

impl SqliteControlPlaneRepository {
    pub(crate) async fn reserve_codex_operation(
        &self,
        id: Uuid,
        kind: &'static str,
    ) -> Result<Option<(CodexCredentialRecord, SqliteCodexOperation)>, RepositoryError> {
        let owner = Arc::clone(&self.database.pools().map_err(open_failure)?.owner);
        let lock = credential_lock(&self.database, id).lock_owned().await;
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        let record = sqlx::query_as::<_, CodexCredentialRecordRow>(sqlx::AssertSqlSafe(
            credential_select("WHERE c.channel_id=? AND c.deleted_at IS NULL"),
        ))
        .bind(SqliteUuid(id))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(record) = record else {
            return Ok(None);
        };
        let record = record.0;
        let attempt = Uuid::new_v4();
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM _gateway_codex_operations WHERE credential_id=?)",
        )
        .bind(SqliteUuid(id))
        .fetch_one(&mut *tx)
        .await?;
        if pending {
            return Err(RepositoryError::Conflict);
        }
        tx.commit().await?;
        let operation = SqliteCodexOperation {
            database: Arc::clone(&self.database),
            channel: id,
            attempt,
            generation: record.refresh_generation,
            credits: record.quota_reset_credits_available,
            updated_at: record.updated_at,
            started_at: None,
            kind,
            _lock: lock,
            _owner: owner,
        };
        Ok(Some((record, operation)))
    }
}

impl SqliteCodexOperation {
    pub(crate) async fn prepare_dispatch(&mut self) -> Result<(), RepositoryError> {
        if self.started_at.is_some() {
            return Err(RepositoryError::Conflict);
        }
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        let started = sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO _gateway_codex_operations(credential_id,attempt_id,kind,generation)
             SELECT channel_id,?,?,refresh_generation FROM codex_oauth_credentials
             WHERE channel_id=? AND refresh_generation=? AND updated_at=? AND deleted_at IS NULL
             ON CONFLICT(credential_id) DO NOTHING RETURNING started_at",
        )
        .bind(SqliteUuid(self.attempt))
        .bind(self.kind)
        .bind(SqliteUuid(self.channel))
        .bind(self.generation)
        .bind(SqliteTimestamp(self.updated_at))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RepositoryError::Conflict)?
        .0;
        tx.commit().await?;
        self.started_at = Some(started);
        Ok(())
    }
    async fn release(&self, tx: &mut Transaction<'_, Sqlite>) -> Result<(), RepositoryError> {
        if self.started_at.is_none() {
            return Err(RepositoryError::Conflict);
        }
        let removed = sqlx::query(
            "DELETE FROM _gateway_codex_operations WHERE credential_id=?1 AND attempt_id=?2
             AND generation=?3 AND EXISTS(SELECT 1 FROM codex_oauth_credentials
               WHERE channel_id=?1 AND refresh_generation=?3 AND deleted_at IS NULL)",
        )
        .bind(SqliteUuid(self.channel))
        .bind(SqliteUuid(self.attempt))
        .bind(self.generation)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if removed != 1 {
            return Err(RepositoryError::Conflict);
        }
        Ok(())
    }
    pub(crate) async fn unchanged(self) -> Result<(), RepositoryError> {
        if self.started_at.is_some() {
            return Err(RepositoryError::Conflict);
        }
        Ok(())
    }
    pub(crate) async fn complete_refresh(
        self,
        update: CodexTokenRefreshUpdate,
    ) -> Result<(), RepositoryError> {
        if update.expected_generation != self.generation {
            return Err(RepositoryError::Conflict);
        }
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        self.release(&mut tx).await?;
        let repo = SqliteControlPlaneRepository::new(Arc::clone(&self.database));
        if !repo
            .persist_codex_token_refresh_transaction(&mut tx, self.channel, update)
            .await?
        {
            return Err(RepositoryError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn fail(
        self,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        self.release(&mut tx).await?;
        SqliteControlPlaneRepository::new(Arc::clone(&self.database))
            .mark_codex_credential_error_transaction(
                &mut tx,
                self.channel,
                permanent,
                code,
                summary,
            )
            .await?;
        // Even a reported provider error may follow rotation. Explicit reauthorization, not
        // automatic expiry or retry, is required to replace the unresolved refresh intent.
        sqlx::query("INSERT INTO _gateway_codex_operations VALUES (?,?,?,?,?)")
            .bind(SqliteUuid(self.channel))
            .bind(SqliteUuid(self.attempt))
            .bind(self.kind)
            .bind(self.generation)
            .bind(self.started_at.map(SqliteTimestamp))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn complete_reset(
        self,
        actor: Uuid,
        event: Uuid,
        requested: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows: i32,
    ) -> Result<Uuid, RepositoryError> {
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        self.release(&mut tx).await?;
        let correlation = SqliteControlPlaneRepository::new(Arc::clone(&self.database))
            .record_codex_quota_reset_transaction(
                &mut tx,
                actor,
                self.channel,
                event,
                requested,
                outcome,
                windows,
                self.credits,
            )
            .await?;
        tx.commit().await?;
        Ok(correlation)
    }
}

pub(super) async fn reauthorize(
    repository: &SqliteControlPlaneRepository,
    tx: &mut Transaction<'_, Sqlite>,
    id: Uuid,
) -> Result<Option<OwnedMutexGuard<()>>, RepositoryError> {
    let kind: Option<String> =
        sqlx::query_scalar("SELECT kind FROM _gateway_codex_operations WHERE credential_id=?")
            .bind(SqliteUuid(id))
            .fetch_optional(&mut **tx)
            .await?;
    let Some(kind) = kind else { return Ok(None) };
    if kind != "refresh" {
        return Err(RepositoryError::Conflict);
    }
    let lock = credential_lock(&repository.database, id)
        .try_lock_owned()
        .map_err(|_| RepositoryError::Conflict)?;
    sqlx::query("DELETE FROM _gateway_codex_operations WHERE credential_id=? AND kind='refresh'")
        .bind(SqliteUuid(id))
        .execute(&mut **tx)
        .await?;
    Ok(Some(lock))
}
