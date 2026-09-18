//! Locked credential operations; external provider calls remain in application.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::persistence::*;

/// Keeps the credential locked until a refresh outcome is committed or dropped.
pub struct CodexRefresh<'a> {
    repository: &'a PostgresControlPlaneRepository,
    transaction: Transaction<'a, Postgres>,
    channel_id: Uuid,
}

pub struct CodexQuotaReset<'a> {
    repository: &'a PostgresControlPlaneRepository,
    transaction: Transaction<'a, Postgres>,
    channel_id: Uuid,
    credits_available: Option<i64>,
}

impl PostgresControlPlaneRepository {
    pub async fn lock_codex_refresh(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<(CodexCredentialRecord, CodexRefresh<'_>)>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let Some(record) = self
            .codex_credential_for_update(&mut transaction, channel_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some((
            record,
            CodexRefresh {
                repository: self,
                transaction,
                channel_id,
            },
        )))
    }

    pub async fn lock_codex_quota_reset(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<(CodexCredentialRecord, CodexQuotaReset<'_>)>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let Some(record) = self
            .codex_credential_for_update(&mut transaction, channel_id)
            .await?
        else {
            return Ok(None);
        };
        let credits_available = record.quota_reset_credits_available;
        Ok(Some((
            record,
            CodexQuotaReset {
                repository: self,
                transaction,
                channel_id,
                credits_available,
            },
        )))
    }
}

impl CodexRefresh<'_> {
    pub async fn unchanged(self) -> Result<(), RepositoryError> {
        self.transaction.commit().await?;
        Ok(())
    }

    pub async fn complete(
        mut self,
        update: CodexTokenRefreshUpdate,
    ) -> Result<(), RepositoryError> {
        if !self
            .repository
            .persist_codex_token_refresh_transaction(&mut self.transaction, self.channel_id, update)
            .await?
        {
            return Err(RepositoryError::Conflict);
        }
        self.transaction.commit().await?;
        Ok(())
    }

    pub async fn fail(
        mut self,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        self.repository
            .mark_codex_credential_error_transaction(
                &mut self.transaction,
                self.channel_id,
                permanent,
                code,
                summary,
            )
            .await?;
        self.transaction.commit().await?;
        Ok(())
    }
}

impl CodexQuotaReset<'_> {
    pub async fn complete(
        mut self,
        actor: Uuid,
        event_id: Uuid,
        requested_at: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows_reset: i32,
    ) -> Result<Uuid, RepositoryError> {
        let correlation_id = self
            .repository
            .record_codex_quota_reset_transaction(
                &mut self.transaction,
                actor,
                self.channel_id,
                event_id,
                requested_at,
                outcome,
                windows_reset,
                self.credits_available,
            )
            .await?;
        self.transaction.commit().await?;
        Ok(correlation_id)
    }
}
