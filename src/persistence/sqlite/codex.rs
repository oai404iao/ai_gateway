//! SQLite Codex credential mutations and quota-window reconciliation.
mod io;
pub(crate) mod operation;

use super::{SqliteControlPlaneRepository, SqliteTimestamp, SqliteUuid};
use crate::persistence::*;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, Row, Sqlite, SqliteConnection, Transaction};
use std::collections::{BTreeSet, HashSet};
use uuid::Uuid;

const CODEX_CONNECTOR_KIND: &str = "codex_oauth";
const QUOTA_WINDOW_IDENTITY_TOLERANCE: Duration = Duration::seconds(90);
const MANUAL_RESET_MATCH_WINDOW: Duration = Duration::minutes(15);

struct CodexPoolContext {
    connector_pool_id: Uuid,
    responses_channel_group_id: Uuid,
}

impl<'r> FromRow<'r, sqlx::sqlite::SqliteRow> for CodexPoolContext {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(CodexPoolContext {
            connector_pool_id: row.try_get::<SqliteUuid, _>("connector_pool_id")?.0,
            responses_channel_group_id: row
                .try_get::<SqliteUuid, _>("responses_channel_group_id")?
                .0,
        })
    }
}

#[derive(Clone, Copy)]
enum CodexQuotaWindowKind {
    Primary,
    Secondary,
}

struct CurrentCodexQuotaWindowPeriod {
    id: Uuid,
    window_seconds: i32,
    started_at: DateTime<Utc>,
    scheduled_reset_at: DateTime<Utc>,
    initial_used_percent: i32,
    last_used_percent: i32,
    last_observed_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, sqlx::sqlite::SqliteRow> for CurrentCodexQuotaWindowPeriod {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(CurrentCodexQuotaWindowPeriod {
            id: row.try_get::<SqliteUuid, _>("id")?.0,
            window_seconds: row.try_get::<i32, _>("window_seconds")?,
            started_at: row.try_get::<SqliteTimestamp, _>("started_at")?.0,
            scheduled_reset_at: row.try_get::<SqliteTimestamp, _>("scheduled_reset_at")?.0,
            initial_used_percent: row.try_get::<i32, _>("initial_used_percent")?,
            last_used_percent: row.try_get::<i32, _>("last_used_percent")?,
            last_observed_at: row.try_get::<SqliteTimestamp, _>("last_observed_at")?.0,
        })
    }
}

struct ObservedCodexQuotaWindow {
    used_percent: i32,
    window_seconds: i32,
    reset_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, sqlx::sqlite::SqliteRow> for ObservedCodexQuotaWindow {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(ObservedCodexQuotaWindow {
            used_percent: row.try_get::<i32, _>("used_percent")?,
            window_seconds: row.try_get::<i32, _>("window_seconds")?,
            reset_at: row.try_get::<SqliteTimestamp, _>("reset_at")?.0,
        })
    }
}

impl CodexQuotaWindowKind {
    const ALL: [Self; 2] = [Self::Primary, Self::Secondary];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }
}

impl SqliteControlPlaneRepository {
    pub(super) async fn insert_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<MutationResult, RepositoryError> {
        validate_credential_settings(&input.label, input.quota_threshold_percent)?;
        let requested_channel_group_id = input.channel_group_id;
        let pool = validate_codex_group_and_proxy_transaction(
            transaction,
            requested_channel_group_id,
            input.proxy_id,
        )
        .await?;
        let input = CodexCredentialCreate {
            channel_group_id: pool.responses_channel_group_id,
            ..input
        };
        if input
            .account_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 300)
            || input
                .user_id
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 300)
            || (input.account_id.is_none() && input.user_id.is_none())
            || input.id_token.is_empty()
            || input.access_token.is_empty()
            || input.refresh_token.is_empty()
            || input.available_models.is_empty()
        {
            return Err(RepositoryError::Validation);
        }
        let existing_channel_id = existing_codex_channel_id(
            transaction,
            pool.connector_pool_id,
            input.account_id.as_deref(),
            input.user_id.as_deref(),
            input.email.as_deref(),
        )
        .await?;
        if let Some(flow_id) = oauth_flow_id {
            let updated = sqlx::query(
                "UPDATE codex_oauth_flows SET completed_at=ag_now() \
                 WHERE id=?1 AND channel_group_id=?2 AND completed_at IS NULL AND expires_at>ag_now()",
            )
            .bind(SqliteUuid(flow_id))
            .bind(SqliteUuid(requested_channel_group_id))
            .execute(&mut **transaction)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(RepositoryError::Conflict);
            }
        }

        if let Some(channel_id) = existing_channel_id {
            let _reauthorization = operation::reauthorize(self, transaction, channel_id).await?;
            let before = codex_credential_audit(transaction, channel_id).await?;
            let existing_quota_checked_at = sqlx::query_scalar::<_, Option<SqliteTimestamp>>(
                "SELECT quota_checked_at FROM codex_oauth_credentials WHERE channel_id=?1",
            )
            .bind(SqliteUuid(channel_id))
            .fetch_one(&mut **transaction)
            .await?;
            crate::persistence::upstream_topology::codex::sqlite_reconfigure(
                transaction,
                channel_id,
                &input.label,
                Some(&input.base_url),
                input.proxy_id,
            )
            .await?;
            crate::persistence::upstream_topology::codex::sqlite_update_credential_lifecycle(
                transaction,
                channel_id,
                input.enabled,
                true,
            )
            .await?;

            let quota = input.quota.as_ref().filter(|quota| {
                existing_quota_checked_at.is_none_or(|checked_at| quota.checked_at >= checked_at.0)
            });
            let has_quota = quota.is_some();
            let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
                "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
                 label=?2,email=?3,plan_type=?4,is_fedramp=?5,id_token=?6,access_token=?7, \
                 refresh_token=?8,access_token_expires_at=?9,last_refreshed_at=?10, \
                 refresh_generation=refresh_generation+1,reauth_required=false,enabled=?12, \
                 quota_threshold_percent=?11,runtime_status=CASE \
                     WHEN NOT ?12 THEN 'disabled' \
                     WHEN ?13 THEN CASE \
                         WHEN NOT ?14 OR ?15 THEN 'unavailable' \
                         WHEN MAX(COALESCE(?16,0),COALESCE(?19,0)) >= ?11 \
                             THEN 'draining' \
                         ELSE 'active' END \
                     WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                     WHEN MAX(COALESCE(primary_used_percent,0), \
                                   COALESCE(secondary_used_percent,0)) >= ?11 \
                         THEN 'draining' \
                     ELSE 'active' END, \
                 quota_allowed=CASE WHEN ?13 THEN ?14 ELSE quota_allowed END, \
                 quota_limit_reached=CASE WHEN ?13 THEN ?15 ELSE quota_limit_reached END, \
                 primary_used_percent=CASE WHEN ?13 THEN ?16 ELSE primary_used_percent END, \
                 primary_window_seconds=CASE WHEN ?13 THEN ?17 ELSE primary_window_seconds END, \
                 primary_reset_at=CASE WHEN ?13 THEN ?18 ELSE primary_reset_at END, \
                 secondary_used_percent=CASE WHEN ?13 THEN ?19 ELSE secondary_used_percent END, \
                 secondary_window_seconds=CASE WHEN ?13 THEN ?20 ELSE secondary_window_seconds END, \
                 secondary_reset_at=CASE WHEN ?13 THEN ?21 ELSE secondary_reset_at END, \
                 quota_checked_at=CASE WHEN ?13 THEN ?22 ELSE quota_checked_at END, \
                 quota_reset_credits_available=CASE \
                     WHEN ?13 THEN ?23 ELSE quota_reset_credits_available END, \
                 last_error_code=NULL,last_error_summary=NULL, \
                 user_id=COALESCE(?24,user_id) \
                 WHERE channel_id=?1 AND deleted_at IS NULL \
                 RETURNING updated_at",
            )
            .bind(SqliteUuid(channel_id))
            .bind(input.label.trim())
            .bind(input.email)
            .bind(input.plan_type)
            .bind(input.is_fedramp)
            .bind(input.id_token)
            .bind(input.access_token)
            .bind(input.refresh_token)
            .bind(input.access_token_expires_at.map(SqliteTimestamp))
            .bind(SqliteTimestamp(Utc::now()))
            .bind(input.quota_threshold_percent)
            .bind(input.enabled)
            .bind(has_quota)
            .bind(quota.map(|quota| quota.allowed))
            .bind(quota.map(|quota| quota.limit_reached))
            .bind(quota.and_then(|quota| quota.primary_used_percent))
            .bind(quota.and_then(|quota| quota.primary_window_seconds))
            .bind(quota.and_then(|quota| quota.primary_reset_at).map(SqliteTimestamp))
            .bind(quota.and_then(|quota| quota.secondary_used_percent))
            .bind(quota.and_then(|quota| quota.secondary_window_seconds))
            .bind(quota.and_then(|quota| quota.secondary_reset_at).map(SqliteTimestamp))
            .bind(quota.map(|quota| quota.checked_at).map(SqliteTimestamp))
            .bind(quota.and_then(|quota| quota.reset_credits_available))
            .bind(input.user_id)
            .fetch_one(&mut **transaction)
            .await?;
            if let Some(quota) = quota {
                reconcile_codex_quota_windows(transaction, channel_id, quota).await?;
            }

            return Ok(MutationResult {
                id: channel_id,
                object_type: "codex_oauth_credential",
                action: "update",
                before_redacted: before,
                after_redacted: codex_credential_audit(transaction, channel_id).await?,
                created_secret: None,
                reason: None,
                updated_at: updated_at.0,
                correlation_id: None,
            });
        }

        let channel_id = Uuid::new_v4();
        crate::persistence::upstream_topology::codex::sqlite_create(
            transaction,
            crate::persistence::upstream_topology::codex::CodexCanonicalCreate {
                credential_id: channel_id,
                group_id: input.channel_group_id,
                label: input.label.trim(),
                base_url: &input.base_url,
                proxy_id: input.proxy_id,
                enabled: input.enabled,
                available_models: &input.available_models,
            },
        )
        .await?;

        let quota = input.quota.as_ref();
        let runtime_status = if input.enabled {
            quota.map_or("active", |quota| {
                runtime_status_for_quota(quota, input.quota_threshold_percent)
            })
        } else {
            "disabled"
        };
        let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO codex_oauth_credentials \
             (channel_id,channel_group_id,connector_pool_id,label,email,account_id,user_id,plan_type,is_fedramp,id_token, \
              access_token,refresh_token,access_token_expires_at,last_refreshed_at, \
              enabled,quota_threshold_percent,runtime_status,quota_allowed,quota_limit_reached, \
              primary_used_percent,primary_window_seconds,primary_reset_at, \
              secondary_used_percent,secondary_window_seconds,secondary_reset_at, \
              quota_reset_credits_available,quota_checked_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27) \
             RETURNING updated_at",
        )
        .bind(SqliteUuid(channel_id))
        .bind(SqliteUuid(input.channel_group_id))
        .bind(SqliteUuid(pool.connector_pool_id))
        .bind(input.label.trim())
        .bind(input.email)
        .bind(input.account_id)
        .bind(input.user_id)
        .bind(input.plan_type)
        .bind(input.is_fedramp)
        .bind(input.id_token)
        .bind(input.access_token)
        .bind(input.refresh_token)
        .bind(input.access_token_expires_at.map(SqliteTimestamp))
        .bind(SqliteTimestamp(Utc::now()))
        .bind(input.enabled)
        .bind(input.quota_threshold_percent)
        .bind(runtime_status)
        .bind(quota.map(|quota| quota.allowed))
        .bind(quota.map(|quota| quota.limit_reached))
        .bind(quota.and_then(|quota| quota.primary_used_percent))
        .bind(quota.and_then(|quota| quota.primary_window_seconds))
        .bind(quota.and_then(|quota| quota.primary_reset_at).map(SqliteTimestamp))
        .bind(quota.and_then(|quota| quota.secondary_used_percent))
        .bind(quota.and_then(|quota| quota.secondary_window_seconds))
        .bind(quota.and_then(|quota| quota.secondary_reset_at).map(SqliteTimestamp))
        .bind(quota.and_then(|quota| quota.reset_credits_available))
        .bind(quota.map(|quota| quota.checked_at).map(SqliteTimestamp))
        .fetch_one(&mut **transaction)
        .await?;
        if let Some(quota) = quota {
            reconcile_codex_quota_windows(transaction, channel_id, quota).await?;
        }

        Ok(MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "create",
            before_redacted: json!({}),
            after_redacted: codex_credential_audit(transaction, channel_id).await?,
            created_secret: None,
            reason: None,
            updated_at: updated_at.0,
            correlation_id: None,
        })
    }

    pub(super) async fn update_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, RepositoryError> {
        validate_credential_settings(&input.label, input.quota_threshold_percent)?;
        let before = codex_credential_audit(transaction, channel_id).await?;
        let group_id = before["channel_group_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(RepositoryError::Validation)?;
        validate_codex_group_and_proxy_transaction(transaction, group_id, input.proxy_id).await?;
        let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE codex_oauth_credentials \
             SET updated_at=ag_now(),label=?2,quota_threshold_percent=?3,enabled=?4,runtime_status=CASE \
                 WHEN NOT ?4 THEN 'disabled' \
                 WHEN reauth_required THEN 'unavailable' \
                 WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                 WHEN MAX(COALESCE(primary_used_percent,0), \
                               COALESCE(secondary_used_percent,0)) >= ?3 \
                     THEN 'draining' \
                 ELSE 'active' END \
             WHERE channel_id=?1 AND updated_at=?5 AND deleted_at IS NULL \
             RETURNING updated_at",
        )
        .bind(SqliteUuid(channel_id))
        .bind(input.label.trim())
        .bind(input.quota_threshold_percent)
        .bind(input.enabled)
        .bind(SqliteTimestamp(expected_updated_at))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        crate::persistence::upstream_topology::codex::sqlite_reconfigure(
            transaction,
            channel_id,
            &input.label,
            None,
            input.proxy_id,
        )
        .await?;
        crate::persistence::upstream_topology::codex::sqlite_update_credential_lifecycle(
            transaction,
            channel_id,
            input.enabled,
            false,
        )
        .await?;

        Ok(MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "update",
            before_redacted: before,
            after_redacted: codex_credential_audit(transaction, channel_id).await?,
            created_secret: None,
            reason: None,
            updated_at: updated_at.0,
            correlation_id: None,
        })
    }

    pub(super) async fn delete_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        channel_id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, RepositoryError> {
        delete_codex_credential(transaction, channel_id, None, expected_updated_at).await
    }

    pub(super) async fn update_codex_credentials_batch(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        const MAX_BATCH_SIZE: usize = 100;

        if input.items.is_empty() || input.items.len() > MAX_BATCH_SIZE {
            return Err(RepositoryError::Validation);
        }
        let pool =
            validate_codex_group_and_proxy_transaction(transaction, channel_group_id, None).await?;
        let mut ids = HashSet::with_capacity(input.items.len());
        if input.items.iter().any(|item| !ids.insert(item.id)) {
            return Err(RepositoryError::Validation);
        }

        let mut results = Vec::with_capacity(input.items.len());
        for item in input.items {
            let result = match input.operation {
                CodexCredentialBatchOperation::Enable => {
                    set_codex_credential_enabled(
                        transaction,
                        item.id,
                        pool.connector_pool_id,
                        item.updated_at,
                        true,
                    )
                    .await?
                }
                CodexCredentialBatchOperation::Disable => {
                    set_codex_credential_enabled(
                        transaction,
                        item.id,
                        pool.connector_pool_id,
                        item.updated_at,
                        false,
                    )
                    .await?
                }
                CodexCredentialBatchOperation::Delete => {
                    delete_codex_credential(
                        transaction,
                        item.id,
                        Some(pool.connector_pool_id),
                        item.updated_at,
                    )
                    .await?
                }
            };
            results.push(result);
        }
        Ok(results)
    }

    pub(super) async fn persist_codex_token_refresh_transaction(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        channel_id: Uuid,
        update: CodexTokenRefreshUpdate,
    ) -> Result<bool, RepositoryError> {
        let updated = sqlx::query(
            "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
             id_token=COALESCE(?3,id_token),access_token=COALESCE(?4,access_token), \
             refresh_token=COALESCE(?5,refresh_token),email=COALESCE(?6,email), \
             account_id=COALESCE(?7,account_id),plan_type=COALESCE(?8,plan_type), \
             is_fedramp=COALESCE(?9,is_fedramp),access_token_expires_at=?10, \
             last_refreshed_at=?11,refresh_generation=refresh_generation+1, \
             user_id=COALESCE(?12,user_id), \
             reauth_required=false, \
             runtime_status=CASE \
                 WHEN NOT enabled THEN 'disabled' \
                 WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                 WHEN MAX(COALESCE(primary_used_percent,0), \
                               COALESCE(secondary_used_percent,0)) >= quota_threshold_percent \
                     THEN 'draining' \
                 ELSE 'active' END, \
             last_error_code=NULL,last_error_summary=NULL \
             WHERE channel_id=?1 AND refresh_generation=?2 AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(channel_id))
        .bind(update.expected_generation)
        .bind(update.id_token)
        .bind(update.access_token)
        .bind(update.refresh_token)
        .bind(update.email)
        .bind(update.account_id)
        .bind(update.plan_type)
        .bind(update.is_fedramp)
        .bind(update.access_token_expires_at.map(SqliteTimestamp))
        .bind(SqliteTimestamp(update.refreshed_at))
        .bind(update.user_id)
        .execute(&mut **transaction)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_codex_quota_reset_transaction(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        actor_user_id: Uuid,
        channel_id: Uuid,
        event_id: Uuid,
        requested_at: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows_reset: i32,
        reset_credits_available: Option<i64>,
    ) -> Result<Uuid, RepositoryError> {
        if !(0..=2).contains(&windows_reset) {
            return Err(RepositoryError::Validation);
        }
        let correlation_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO codex_quota_reset_events \
             (id,credential_id,actor_user_id,requested_at,outcome,windows_reset,correlation_id) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )
        .bind(SqliteUuid(event_id))
        .bind(SqliteUuid(channel_id))
        .bind(SqliteUuid(actor_user_id))
        .bind(SqliteTimestamp(requested_at))
        .bind(outcome.as_str())
        .bind(windows_reset)
        .bind(SqliteUuid(correlation_id))
        .execute(&mut **transaction)
        .await?;
        let mutation = MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "reset_quota",
            before_redacted: json!({
                "quota_reset_credits_available": reset_credits_available,
            }),
            after_redacted: json!({
                "outcome": outcome.as_str(),
                "windows_reset": windows_reset,
                "redeem_request_id": event_id,
            }),
            created_secret: None,
            reason: Some("manual_reset_credit".into()),
            updated_at: requested_at,
            correlation_id: Some(correlation_id),
        };
        super::control_plane::insert_user_audit(
            transaction,
            actor_user_id,
            "admin",
            &mutation,
            correlation_id,
        )
        .await?;
        Ok(correlation_id)
    }

    pub(super) async fn mark_codex_credential_error_transaction(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        channel_id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
             reauth_required=reauth_required OR ?2, \
             runtime_status=CASE WHEN ?2 THEN 'unavailable' ELSE runtime_status END, \
             last_error_code=CASE WHEN reauth_required AND NOT ?2 THEN last_error_code ELSE ?3 END, \
             last_error_summary=CASE WHEN reauth_required AND NOT ?2 THEN last_error_summary ELSE ?4 END \
             WHERE channel_id=?1 AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(channel_id))
        .bind(permanent)
        .bind(code)
        .bind(summary)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }
}

async fn reconcile_codex_quota_windows(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
    quota: &CodexQuotaUpdate,
) -> Result<(), RepositoryError> {
    for kind in CodexQuotaWindowKind::ALL {
        let Some(observation) = observed_codex_quota_window(quota, kind)? else {
            continue;
        };
        reconcile_codex_quota_window(transaction, channel_id, kind, observation, quota.checked_at)
            .await?;
    }
    Ok(())
}

fn observed_codex_quota_window(
    quota: &CodexQuotaUpdate,
    kind: CodexQuotaWindowKind,
) -> Result<Option<ObservedCodexQuotaWindow>, RepositoryError> {
    let values = match kind {
        CodexQuotaWindowKind::Primary => (
            quota.primary_used_percent,
            quota.primary_window_seconds,
            quota.primary_reset_at,
        ),
        CodexQuotaWindowKind::Secondary => (
            quota.secondary_used_percent,
            quota.secondary_window_seconds,
            quota.secondary_reset_at,
        ),
    };
    match values {
        (None, None, None) => Ok(None),
        (Some(used_percent), Some(window_seconds), Some(reset_at))
            if (0..=100).contains(&used_percent) && window_seconds > 0 =>
        {
            Ok(Some(ObservedCodexQuotaWindow {
                used_percent,
                window_seconds,
                reset_at,
            }))
        }
        _ => Err(RepositoryError::Validation),
    }
}

async fn reconcile_codex_quota_window(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
    kind: CodexQuotaWindowKind,
    observation: ObservedCodexQuotaWindow,
    checked_at: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let started_at = observation
        .reset_at
        .checked_sub_signed(Duration::seconds(i64::from(observation.window_seconds)))
        .ok_or(RepositoryError::Validation)?;
    let current = sqlx::query_as::<_, CurrentCodexQuotaWindowPeriod>(
        "SELECT id,window_seconds,started_at,scheduled_reset_at, \
                initial_used_percent,last_used_percent,last_observed_at \
         FROM codex_quota_window_periods \
         WHERE credential_id=?1 AND window_kind=?2 AND ended_at IS NULL \
        ",
    )
    .bind(SqliteUuid(channel_id))
    .bind(kind.as_str())
    .fetch_optional(&mut **transaction)
    .await?;

    let Some(current) = current else {
        insert_codex_quota_window_period(
            transaction,
            channel_id,
            kind,
            observation,
            started_at,
            checked_at,
        )
        .await?;
        return Ok(());
    };

    if checked_at < current.last_observed_at {
        return Ok(());
    }

    let same_period = current.window_seconds == observation.window_seconds
        && timestamps_within(
            current.scheduled_reset_at,
            observation.reset_at,
            QUOTA_WINDOW_IDENTITY_TOLERANCE,
        );
    if same_period
        || started_at
            <= current
                .started_at
                .checked_add_signed(QUOTA_WINDOW_IDENTITY_TOLERANCE)
                .unwrap_or(current.started_at)
    {
        sqlx::query(
            "UPDATE codex_quota_window_periods \
             SET updated_at=ag_now(),last_used_percent=CASE \
                     WHEN last_observed_at <= ?3 THEN ?2 \
                     ELSE last_used_percent \
                 END, \
                 last_observed_at=MAX(last_observed_at,?3) \
             WHERE id=?1",
        )
        .bind(SqliteUuid(current.id))
        .bind(observation.used_percent)
        .bind(SqliteTimestamp(checked_at))
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }

    let natural_boundary = current
        .scheduled_reset_at
        .checked_sub_signed(QUOTA_WINDOW_IDENTITY_TOLERANCE)
        .unwrap_or(current.scheduled_reset_at);
    let manual_reset = if started_at < natural_boundary {
        claim_manual_codex_quota_reset(transaction, channel_id, kind, started_at, checked_at)
            .await?
    } else {
        false
    };

    if current.initial_used_percent == 0
        && current.last_used_percent == 0
        && started_at < natural_boundary
        && !manual_reset
    {
        sqlx::query(
            "UPDATE codex_quota_window_periods \
             SET updated_at=ag_now(),window_seconds=?2,started_at=?3,scheduled_reset_at=?4, \
                 last_used_percent=?5,last_observed_at=MAX(last_observed_at,?6) \
             WHERE id=?1",
        )
        .bind(SqliteUuid(current.id))
        .bind(observation.window_seconds)
        .bind(SqliteTimestamp(started_at))
        .bind(SqliteTimestamp(observation.reset_at))
        .bind(observation.used_percent)
        .bind(SqliteTimestamp(checked_at))
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }

    let (reset_reason, ended_at) = if started_at >= natural_boundary {
        ("natural", current.scheduled_reset_at)
    } else if manual_reset {
        ("manual", started_at)
    } else {
        ("openai_official", started_at)
    };
    sqlx::query(
        "UPDATE codex_quota_window_periods \
         SET updated_at=ag_now(),ended_at=?2,reset_reason=?3 \
         WHERE id=?1",
    )
    .bind(SqliteUuid(current.id))
    .bind(SqliteTimestamp(ended_at))
    .bind(reset_reason)
    .execute(&mut **transaction)
    .await?;
    insert_codex_quota_window_period(
        transaction,
        channel_id,
        kind,
        observation,
        started_at,
        checked_at,
    )
    .await
}

async fn insert_codex_quota_window_period(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
    kind: CodexQuotaWindowKind,
    observation: ObservedCodexQuotaWindow,
    started_at: DateTime<Utc>,
    checked_at: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO codex_quota_window_periods \
         (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at, \
          initial_used_percent,last_used_percent,first_observed_at,last_observed_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?7,?8,?8)",
    )
    .bind(SqliteUuid(Uuid::new_v4()))
    .bind(SqliteUuid(channel_id))
    .bind(kind.as_str())
    .bind(observation.window_seconds)
    .bind(SqliteTimestamp(started_at))
    .bind(SqliteTimestamp(observation.reset_at))
    .bind(observation.used_percent)
    .bind(SqliteTimestamp(checked_at))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn claim_manual_codex_quota_reset(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
    kind: CodexQuotaWindowKind,
    transition_started_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> Result<bool, RepositoryError> {
    let event_id = sqlx::query_scalar::<_, SqliteUuid>(
        "SELECT id \
         FROM codex_quota_reset_events \
         WHERE credential_id=?1 \
           AND outcome IN ('reset','already_redeemed') \
           AND windows_reset > ( \
               CASE WHEN primary_applied_at IS NULL THEN 0 ELSE 1 END \
               + CASE WHEN secondary_applied_at IS NULL THEN 0 ELSE 1 END \
           ) \
           AND requested_at <= ?2 \
           AND requested_at >= ?3 \
           AND CASE \
               WHEN ?4='primary' THEN primary_applied_at \
               ELSE secondary_applied_at \
           END IS NULL \
         ORDER BY requested_at DESC,id DESC \
         \
         LIMIT 1",
    )
    .bind(SqliteUuid(channel_id))
    .bind(SqliteTimestamp(
        transition_started_at + MANUAL_RESET_MATCH_WINDOW,
    ))
    .bind(SqliteTimestamp(
        transition_started_at - MANUAL_RESET_MATCH_WINDOW,
    ))
    .bind(kind.as_str())
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(event_id) = event_id else {
        return Ok(false);
    };
    let update = match kind {
        CodexQuotaWindowKind::Primary => {
            "UPDATE codex_quota_reset_events SET primary_applied_at=?2 WHERE id=?1"
        }
        CodexQuotaWindowKind::Secondary => {
            "UPDATE codex_quota_reset_events SET secondary_applied_at=?2 WHERE id=?1"
        }
    };
    sqlx::query(update)
        .bind(event_id)
        .bind(SqliteTimestamp(observed_at))
        .execute(&mut **transaction)
        .await?;
    Ok(true)
}

fn timestamps_within(left: DateTime<Utc>, right: DateTime<Utc>, tolerance: Duration) -> bool {
    left.signed_duration_since(right).abs() <= tolerance
}

async fn existing_codex_channel_id(
    transaction: &mut Transaction<'_, Sqlite>,
    connector_pool_id: Uuid,
    account_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Result<Option<Uuid>, RepositoryError> {
    let Some(account_id) = account_id else {
        let Some(user_id) = user_id else {
            return Err(RepositoryError::Validation);
        };
        return sqlx::query_scalar::<_, SqliteUuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=?1 AND account_id IS NULL AND user_id=?2 \
               AND deleted_at IS NULL \
            ",
        )
        .bind(SqliteUuid(connector_pool_id))
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await
        .map(|value| value.map(|v| v.0))
        .map_err(RepositoryError::from);
    };

    if let Some(user_id) = user_id {
        let exact = sqlx::query_scalar::<_, SqliteUuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=?1 AND account_id=?2 AND user_id=?3 \
               AND deleted_at IS NULL \
            ",
        )
        .bind(SqliteUuid(connector_pool_id))
        .bind(account_id)
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if exact.is_some() {
            return Ok(exact.map(|v| v.0));
        }
    }

    if let Some(email) = email.map(str::trim).filter(|value| !value.is_empty()) {
        // Once the token carries a member ID, email is only a migration bridge
        // for legacy rows that have not been backfilled yet.
        let matches = sqlx::query_scalar::<_, SqliteUuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=?1 AND account_id=?2 \
               AND ag_lower(email)=ag_lower(?3) AND deleted_at IS NULL \
               AND (?4 OR user_id IS NULL) \
             ORDER BY channel_id \
            ",
        )
        .bind(SqliteUuid(connector_pool_id))
        .bind(account_id)
        .bind(email)
        .bind(user_id.is_none())
        .fetch_all(&mut **transaction)
        .await?;
        return match matches.as_slice() {
            [] => Ok(None),
            [channel_id] => Ok(Some(channel_id.0)),
            _ => Err(RepositoryError::Conflict),
        };
    }

    if user_id.is_none() {
        return sqlx::query_scalar::<_, SqliteUuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=?1 AND account_id=?2 AND user_id IS NULL \
               AND deleted_at IS NULL \
            ",
        )
        .bind(SqliteUuid(connector_pool_id))
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await
        .map(|value| value.map(|v| v.0))
        .map_err(RepositoryError::from);
    }

    Ok(None)
}

async fn set_codex_credential_enabled(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
    connector_pool_id: Uuid,
    expected_updated_at: DateTime<Utc>,
    enabled: bool,
) -> Result<MutationResult, RepositoryError> {
    let before = codex_credential_audit(transaction, channel_id).await?;
    let actual_connector_pool_id = before["connector_pool_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(RepositoryError::Validation)?;
    if actual_connector_pool_id != connector_pool_id {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
         enabled=?3,runtime_status=CASE \
             WHEN NOT ?3 THEN 'disabled' \
             WHEN reauth_required THEN 'unavailable' \
             WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
             WHEN MAX(COALESCE(primary_used_percent,0), \
                           COALESCE(secondary_used_percent,0)) >= quota_threshold_percent \
                 THEN 'draining' \
             ELSE 'active' END \
         WHERE channel_id=?1 AND updated_at=?2 AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(SqliteUuid(channel_id))
    .bind(SqliteTimestamp(expected_updated_at))
    .bind(enabled)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    crate::persistence::upstream_topology::codex::sqlite_update_credential_lifecycle(
        transaction,
        channel_id,
        enabled,
        false,
    )
    .await?;
    Ok(MutationResult {
        id: channel_id,
        object_type: "codex_oauth_credential",
        action: "batch_update",
        before_redacted: before,
        after_redacted: codex_credential_audit(transaction, channel_id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn delete_codex_credential(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
    expected_connector_pool_id: Option<Uuid>,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let sharing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM codex_sharing_groups WHERE credential_id=?1)",
    )
    .bind(SqliteUuid(channel_id))
    .fetch_one(&mut **transaction)
    .await?;
    if sharing {
        return Err(RepositoryError::SharingCredentialInUse);
    }
    let before = codex_credential_audit(transaction, channel_id).await?;
    let actual_connector_pool_id = before["connector_pool_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(RepositoryError::Validation)?;
    if expected_connector_pool_id.is_some_and(|expected| expected != actual_connector_pool_id) {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
         enabled=false,runtime_status='disabled',reauth_required=false, \
         id_token='deleted',access_token='deleted',refresh_token='deleted', \
         access_token_expires_at=NULL,quota_allowed=NULL,quota_limit_reached=NULL, \
         primary_used_percent=NULL,primary_window_seconds=NULL,primary_reset_at=NULL, \
         secondary_used_percent=NULL,secondary_window_seconds=NULL,secondary_reset_at=NULL, \
         quota_reset_credits_available=NULL,quota_checked_at=NULL, \
         last_error_code=NULL,last_error_summary=NULL,deleted_at=ag_now() \
         WHERE channel_id=?1 AND updated_at=?2 AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(SqliteUuid(channel_id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    crate::persistence::upstream_topology::codex::sqlite_delete(transaction, channel_id).await?;
    Ok(MutationResult {
        id: channel_id,
        object_type: "codex_oauth_credential",
        action: "delete",
        before_redacted: before,
        after_redacted: json!({}),
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn validate_codex_group_and_proxy_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<CodexPoolContext, RepositoryError> {
    validate_codex_group_and_proxy_connection(transaction, channel_group_id, proxy_id).await
}

async fn validate_codex_group_and_proxy_connection(
    connection: &mut SqliteConnection,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<CodexPoolContext, RepositoryError> {
    if let Some(proxy_id) = proxy_id {
        let valid_proxy = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM proxies WHERE id=?1 AND enabled)",
        )
        .bind(SqliteUuid(proxy_id))
        .fetch_one(&mut *connection)
        .await?;
        if !valid_proxy {
            return Err(RepositoryError::Validation);
        }
    }
    sqlx::query(
        "INSERT INTO connector_pools(id,connector_kind,routing_group_id)
         SELECT ?,'codex_oauth',id FROM routing_groups WHERE id=? AND deleted_at IS NULL
         ON CONFLICT (routing_group_id) DO NOTHING",
    )
    .bind(SqliteUuid(Uuid::new_v4()))
    .bind(SqliteUuid(channel_group_id))
    .execute(&mut *connection)
    .await?;
    codex_pool_context_connection(connection, channel_group_id).await
}

async fn codex_pool_context_connection(
    connection: &mut SqliteConnection,
    channel_group_id: Uuid,
) -> Result<CodexPoolContext, RepositoryError> {
    sqlx::query_as::<_, CodexPoolContext>(
        "SELECT pool.id AS connector_pool_id,selected.id AS responses_channel_group_id
         FROM routing_groups selected JOIN connector_pools pool ON pool.routing_group_id=selected.id
         WHERE selected.id=?1 AND selected.deleted_at IS NULL AND pool.connector_kind=?2",
    )
    .bind(SqliteUuid(channel_group_id))
    .bind(CODEX_CONNECTOR_KIND)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(RepositoryError::Validation)
}

fn validate_credential_settings(
    label: &str,
    quota_threshold_percent: i16,
) -> Result<(), RepositoryError> {
    if label.trim().is_empty() || label.len() > 100 || !(1..=100).contains(&quota_threshold_percent)
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn runtime_status_for_quota(quota: &CodexQuotaUpdate, threshold: i16) -> &'static str {
    if !quota.allowed || quota.limit_reached {
        return "unavailable";
    }
    let used = quota
        .primary_used_percent
        .unwrap_or_default()
        .max(quota.secondary_used_percent.unwrap_or_default());
    if used >= i32::from(threshold) {
        "draining"
    } else {
        "active"
    }
}

struct CodexCredentialRecordRow(CodexCredentialRecord);
impl<'r> FromRow<'r, sqlx::sqlite::SqliteRow> for CodexCredentialRecordRow {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self(CodexCredentialRecord {
            channel_id: row.try_get::<SqliteUuid, _>("channel_id")?.0,
            channel_group_id: row.try_get::<SqliteUuid, _>("channel_group_id")?.0,
            connector_pool_id: row.try_get::<SqliteUuid, _>("connector_pool_id")?.0,
            projection_channel_ids: row
                .try_get::<sqlx::types::Json<Vec<Uuid>>, _>("projection_channel_ids")?
                .0,
            label: row.try_get::<String, _>("label")?,
            email: row.try_get::<Option<String>, _>("email")?,
            account_id: row.try_get::<Option<String>, _>("account_id")?,
            user_id: row.try_get::<Option<String>, _>("user_id")?,
            plan_type: row.try_get::<Option<String>, _>("plan_type")?,
            is_fedramp: row.try_get::<bool, _>("is_fedramp")?,
            id_token: row.try_get::<String, _>("id_token")?,
            access_token: row.try_get::<String, _>("access_token")?,
            refresh_token: row.try_get::<String, _>("refresh_token")?,
            access_token_expires_at: row
                .try_get::<Option<SqliteTimestamp>, _>("access_token_expires_at")?
                .map(|v| v.0),
            last_refreshed_at: row.try_get::<SqliteTimestamp, _>("last_refreshed_at")?.0,
            refresh_generation: row.try_get::<i64, _>("refresh_generation")?,
            reauth_required: row.try_get::<bool, _>("reauth_required")?,
            quota_threshold_percent: row.try_get::<i16, _>("quota_threshold_percent")?,
            runtime_status: row.try_get::<String, _>("runtime_status")?,
            quota_allowed: row.try_get::<Option<bool>, _>("quota_allowed")?,
            quota_limit_reached: row.try_get::<Option<bool>, _>("quota_limit_reached")?,
            primary_used_percent: row.try_get::<Option<i32>, _>("primary_used_percent")?,
            primary_window_seconds: row.try_get::<Option<i32>, _>("primary_window_seconds")?,
            primary_reset_at: row
                .try_get::<Option<SqliteTimestamp>, _>("primary_reset_at")?
                .map(|v| v.0),
            secondary_used_percent: row.try_get::<Option<i32>, _>("secondary_used_percent")?,
            secondary_window_seconds: row.try_get::<Option<i32>, _>("secondary_window_seconds")?,
            secondary_reset_at: row
                .try_get::<Option<SqliteTimestamp>, _>("secondary_reset_at")?
                .map(|v| v.0),
            quota_reset_credits_available: row
                .try_get::<Option<i64>, _>("quota_reset_credits_available")?,
            quota_checked_at: row
                .try_get::<Option<SqliteTimestamp>, _>("quota_checked_at")?
                .map(|v| v.0),
            last_error_code: row.try_get::<Option<String>, _>("last_error_code")?,
            last_error_summary: row.try_get::<Option<String>, _>("last_error_summary")?,
            proxy_id: row
                .try_get::<Option<SqliteUuid>, _>("proxy_id")?
                .map(|v| v.0),
            enabled: row.try_get::<bool, _>("enabled")?,
            available_models: row
                .try_get::<sqlx::types::Json<Vec<String>>, _>("available_models")?
                .0,
            created_at: row.try_get::<SqliteTimestamp, _>("created_at")?.0,
            updated_at: row.try_get::<SqliteTimestamp, _>("updated_at")?.0,
        }))
    }
}

struct CodexOauthFlowRecordRow(CodexOauthFlowRecord);
impl<'r> FromRow<'r, sqlx::sqlite::SqliteRow> for CodexOauthFlowRecordRow {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self(CodexOauthFlowRecord {
            id: row.try_get::<SqliteUuid, _>("id")?.0,
            actor_user_id: row.try_get::<SqliteUuid, _>("actor_user_id")?.0,
            channel_group_id: row.try_get::<SqliteUuid, _>("channel_group_id")?.0,
            label: row.try_get::<String, _>("label")?,
            proxy_id: row
                .try_get::<Option<SqliteUuid>, _>("proxy_id")?
                .map(|v| v.0),
            quota_threshold_percent: row.try_get::<i16, _>("quota_threshold_percent")?,
            redirect_uri: row.try_get::<String, _>("redirect_uri")?,
            state_hash: row.try_get::<Vec<u8>, _>("state_hash")?,
            code_verifier: row.try_get::<String, _>("code_verifier")?,
            expires_at: row.try_get::<SqliteTimestamp, _>("expires_at")?.0,
        }))
    }
}

struct CodexQuotaWindowPeriodViewRow(CodexQuotaWindowPeriodView);
impl<'r> FromRow<'r, sqlx::sqlite::SqliteRow> for CodexQuotaWindowPeriodViewRow {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self(CodexQuotaWindowPeriodView {
            id: row.try_get::<SqliteUuid, _>("id")?.0,
            credential_id: row.try_get::<SqliteUuid, _>("credential_id")?.0,
            window_kind: row.try_get::<String, _>("window_kind")?,
            window_seconds: row.try_get::<i32, _>("window_seconds")?,
            started_at: row.try_get::<SqliteTimestamp, _>("started_at")?.0,
            scheduled_reset_at: row.try_get::<SqliteTimestamp, _>("scheduled_reset_at")?.0,
            ended_at: row
                .try_get::<Option<SqliteTimestamp>, _>("ended_at")?
                .map(|v| v.0),
            reset_reason: row.try_get::<Option<String>, _>("reset_reason")?,
            initial_used_percent: row.try_get::<i32, _>("initial_used_percent")?,
            last_used_percent: row.try_get::<i32, _>("last_used_percent")?,
            first_observed_at: row.try_get::<SqliteTimestamp, _>("first_observed_at")?.0,
            last_observed_at: row.try_get::<SqliteTimestamp, _>("last_observed_at")?.0,
            cost_amount: row.try_get::<super::SqliteAmount, _>("cost_amount")?.0,
        }))
    }
}

fn open_failure(error: super::SqliteOpenError) -> RepositoryError {
    sqlx::Error::Configuration(Box::new(error)).into()
}

fn credential_select(suffix: &str) -> String {
    format!(
        "SELECT c.*,access.proxy_id AS proxy_id,
      capability.available_models AS available_models,
      '[]' AS projection_channel_ids
      FROM codex_oauth_credentials c
      JOIN upstream_channels channel ON channel.id=c.channel_id
      JOIN upstream_accesses access ON access.id=channel.access_id
      LEFT JOIN channel_capabilities capability
        ON capability.channel_id=channel.id AND capability.operation='responses'
       AND capability.deleted_at IS NULL {suffix}"
    )
}

impl SqliteControlPlaneRepository {
    pub async fn codex_credential(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialRecord>, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        Ok(
            sqlx::query_as::<_, CodexCredentialRecordRow>(sqlx::AssertSqlSafe(credential_select(
                "WHERE c.channel_id=? AND c.deleted_at IS NULL",
            )))
            .bind(SqliteUuid(channel_id))
            .fetch_optional(&mut *reader)
            .await?
            .map(|r| r.0),
        )
    }
    pub async fn load_codex_credentials(
        &self,
    ) -> Result<Vec<CodexCredentialRecord>, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        Ok(
            sqlx::query_as::<_, CodexCredentialRecordRow>(sqlx::AssertSqlSafe(credential_select(
                "WHERE c.deleted_at IS NULL ORDER BY c.channel_id",
            )))
            .fetch_all(&mut *reader)
            .await?
            .into_iter()
            .map(|r| r.0)
            .collect(),
        )
    }
    pub async fn prepare_codex_credential_create(
        &self,
        actor: Uuid,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<super::SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut tx = self.admin_write(actor).await?;
        let mutation = self
            .insert_codex_credential(&mut tx, input, oauth_flow_id)
            .await?;
        Ok(self.prepared(
            tx,
            vec![mutation],
            super::control_plane::AuditKind::Admin(actor),
        ))
    }
    pub async fn prepare_codex_credential_update(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<super::SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut tx = self.admin_write(actor).await?;
        let mutation = self
            .update_codex_credential(&mut tx, channel_id, input, expected_updated_at)
            .await?;
        Ok(self.prepared(
            tx,
            vec![mutation],
            super::control_plane::AuditKind::Admin(actor),
        ))
    }
    pub async fn prepare_codex_credential_delete(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<super::SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut tx = self.admin_write(actor).await?;
        let mutation = self
            .delete_codex_credential(&mut tx, channel_id, expected_updated_at)
            .await?;
        Ok(self.prepared(
            tx,
            vec![mutation],
            super::control_plane::AuditKind::Admin(actor),
        ))
    }
    pub async fn prepare_codex_credentials_batch(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<super::SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut tx = self.admin_write(actor).await?;
        let mutations = self
            .update_codex_credentials_batch(&mut tx, channel_group_id, input)
            .await?;
        Ok(self.prepared(tx, mutations, super::control_plane::AuditKind::Admin(actor)))
    }
}

async fn codex_credential_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    channel_id: Uuid,
) -> Result<Value, RepositoryError> {
    let record = sqlx::query_scalar::<_, sqlx::types::Json<Value>>(
        "SELECT json_object(
             'id',c.channel_id,'channel_group_id',ch.group_id,'connector_pool_id',c.connector_pool_id,
             'label',c.label,'email',c.email,'account_id',c.account_id,'user_id',c.user_id,'plan_type',c.plan_type,
             'is_fedramp',json(CASE c.is_fedramp WHEN 1 THEN 'true' ELSE 'false' END),
             'access_token_expires_at',c.access_token_expires_at,'last_refreshed_at',c.last_refreshed_at,
             'quota_threshold_percent',c.quota_threshold_percent,'runtime_status',c.runtime_status,
             'proxy_id',access.proxy_id,'enabled',json(CASE c.enabled WHEN 1 THEN 'true' ELSE 'false' END),
             'access_id',access.id,'access_revision',access.revision,'binding_revision',ch.binding_revision,
             'base_url','[REDACTED]',
             'capabilities',json((
                 SELECT json_group_array(json_object('id',cap.id,'operation',cap.operation,
                     'available_models',json(cap.available_models),'transports',json(cap.transports),
                     'enabled',json(CASE cap.enabled WHEN 1 THEN 'true' ELSE 'false' END)))
                 FROM (SELECT * FROM channel_capabilities WHERE channel_id=ch.id AND deleted_at IS NULL ORDER BY operation) cap
             )),
             'created_at',c.created_at,'updated_at',c.updated_at)
         FROM codex_oauth_credentials c JOIN upstream_channels ch ON ch.id=c.channel_id
         JOIN upstream_accesses access ON access.id=ch.access_id
         WHERE c.channel_id=? AND c.deleted_at IS NULL",
    )
    .bind(SqliteUuid(channel_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(record.0)
}
