//! SQLite Codex reads and short writes.
use super::super::{SqliteAmount, aggregate::CostSum};
use super::*;
use futures_util::TryStreamExt;
use sqlx::Connection;

async fn window_cost(
    connection: &mut SqliteConnection,
    credential: Uuid,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<rust_decimal::Decimal, RepositoryError> {
    let mut sum = CostSum::default();
    let mut rows=sqlx::query_scalar::<_,SqliteAmount>(
        "SELECT f.cost_amount FROM request_metering_facts f
         JOIN channel_identity_registry identity ON identity.id=f.channel_id
         WHERE identity.codex_credential_id=? AND f.cost_amount IS NOT NULL AND f.started_at>=? AND f.started_at<?")
        .bind(SqliteUuid(credential)).bind(SqliteTimestamp(start)).bind(SqliteTimestamp(end))
        .fetch(connection);
    while let Some(amount) = rows.try_next().await? {
        sum.add(amount.0);
    }
    sum.finish()
}
async fn view(
    connection: &mut SqliteConnection,
    r: CodexCredentialRecord,
) -> Result<CodexCredentialView, RepositoryError> {
    let now = sqlx::query_scalar::<_, SqliteTimestamp>("SELECT ag_now()")
        .fetch_one(&mut *connection)
        .await?
        .0;
    let periods = sqlx::query_as::<_, (String, SqliteTimestamp, SqliteTimestamp)>(
        "SELECT window_kind,started_at,scheduled_reset_at FROM codex_quota_window_periods
         WHERE credential_id=? AND ended_at IS NULL",
    )
    .bind(SqliteUuid(r.channel_id))
    .fetch_all(&mut *connection)
    .await?;
    let (mut primary, mut secondary) = (None, None);
    for (kind, start, end) in periods {
        let cost = window_cost(connection, r.channel_id, start.0, end.0.min(now)).await?;
        match kind.as_str() {
            "primary" => primary = Some(cost),
            "secondary" => secondary = Some(cost),
            _ => return Err(RepositoryError::Validation),
        }
    }
    Ok(CodexCredentialView {
        id: r.channel_id,
        channel_group_id: r.channel_group_id,
        label: r.label,
        email: r.email,
        account_id: r.account_id,
        user_id: r.user_id,
        plan_type: r.plan_type,
        is_fedramp: r.is_fedramp,
        access_token_expires_at: r.access_token_expires_at,
        last_refreshed_at: r.last_refreshed_at,
        quota_threshold_percent: r.quota_threshold_percent,
        runtime_status: r.runtime_status,
        quota_allowed: r.quota_allowed,
        quota_limit_reached: r.quota_limit_reached,
        primary_used_percent: r.primary_used_percent,
        primary_window_seconds: r.primary_window_seconds,
        primary_reset_at: r.primary_reset_at,
        primary_window_cost_amount: primary,
        secondary_used_percent: r.secondary_used_percent,
        secondary_window_seconds: r.secondary_window_seconds,
        secondary_reset_at: r.secondary_reset_at,
        secondary_window_cost_amount: secondary,
        quota_reset_credits_available: r.quota_reset_credits_available,
        quota_checked_at: r.quota_checked_at,
        last_error_code: r.last_error_code,
        last_error_summary: r.last_error_summary,
        proxy_id: r.proxy_id,
        enabled: r.enabled,
        available_models: r.available_models,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}
fn self_view(r: CodexCredentialView) -> SelfCodexQuotaCredentialView {
    SelfCodexQuotaCredentialView {
        id: r.id,
        name: r.id.to_string(),
        channel_group_id: r.channel_group_id,
        plan_type: r.plan_type,
        primary_used_percent: r.primary_used_percent,
        primary_window_seconds: r.primary_window_seconds,
        primary_reset_at: r.primary_reset_at,
        primary_window_cost_amount: r.primary_window_cost_amount,
        secondary_used_percent: r.secondary_used_percent,
        secondary_window_seconds: r.secondary_window_seconds,
        secondary_reset_at: r.secondary_reset_at,
        secondary_window_cost_amount: r.secondary_window_cost_amount,
        quota_checked_at: r.quota_checked_at,
    }
}
async fn history(
    connection: &mut SqliteConnection,
    id: Uuid,
    limit: i64,
) -> Result<CodexQuotaWindowHistory, RepositoryError> {
    if !(1..=500).contains(&limit) {
        return Err(RepositoryError::Validation);
    }
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM codex_oauth_credentials WHERE channel_id=? AND deleted_at IS NULL)")
        .bind(SqliteUuid(id)).fetch_one(&mut *connection).await?;
    if !exists {
        return Err(RepositoryError::NotFound);
    }
    let now = sqlx::query_scalar::<_, SqliteTimestamp>("SELECT ag_now()")
        .fetch_one(&mut *connection)
        .await?
        .0;
    let rows=sqlx::query_as::<_,CodexQuotaWindowPeriodViewRow>(
        "WITH ranked AS (SELECT *,row_number() OVER(PARTITION BY window_kind ORDER BY started_at DESC,id DESC) AS window_rank
         FROM codex_quota_window_periods WHERE credential_id=?1
           AND (reset_reason IS NOT 'openai_official' OR initial_used_percent<>0 OR last_used_percent<>0))
         SELECT *,'0' AS cost_amount FROM ranked WHERE window_rank<=?2 ORDER BY window_kind,started_at DESC,id DESC")
        .bind(SqliteUuid(id)).bind(limit).fetch_all(&mut *connection).await?;
    let mut periods = Vec::with_capacity(rows.len());
    for row in rows {
        let mut row = row.0;
        row.cost_amount = window_cost(
            connection,
            id,
            row.started_at,
            row.ended_at.unwrap_or(row.scheduled_reset_at.min(now)),
        )
        .await?;
        periods.push(row);
    }
    Ok(CodexQuotaWindowHistory {
        credential_id: id,
        periods,
    })
}

const VISIBLE:&str="JOIN channel_groups visible_group ON visible_group.connector_pool_id=c.connector_pool_id
    AND visible_group.connector_kind='codex_oauth' AND visible_group.api_format='open_ai_responses'
    JOIN user_group_codex_quota_visibility visibility ON visibility.channel_group_id=visible_group.id
    JOIN users console_user ON console_user.user_group_id=visibility.user_group_id
    WHERE console_user.id=?1 AND console_user.status='active' AND console_user.deleted_at IS NULL
    AND c.deleted_at IS NULL";

impl SqliteControlPlaneRepository {
    pub async fn set_codex_user_id_if_missing(
        &self,
        id: Uuid,
        user_id: &str,
    ) -> Result<bool, RepositoryError> {
        if user_id.trim().is_empty() || user_id.len() > 300 {
            return Err(RepositoryError::Validation);
        }
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        let changed=sqlx::query("UPDATE codex_oauth_credentials AS target SET updated_at=ag_now(),user_id=?2
            WHERE target.channel_id=?1 AND target.user_id IS NULL AND target.deleted_at IS NULL AND NOT EXISTS(
            SELECT 1 FROM codex_oauth_credentials existing WHERE existing.connector_pool_id=target.connector_pool_id
              AND existing.account_id IS target.account_id AND existing.user_id=?2 AND existing.deleted_at IS NULL)")
            .bind(SqliteUuid(id)).bind(user_id.trim()).execute(&mut *tx).await?.rows_affected()==1;
        tx.commit().await?;
        Ok(changed)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn create_codex_oauth_flow(
        &self,
        actor: Uuid,
        group: Uuid,
        input: CodexOauthStartInput,
        redirect_uri: String,
        state_hash: Vec<u8>,
        code_verifier: String,
        expires_at: DateTime<Utc>,
    ) -> Result<CodexOauthFlowRecord, RepositoryError> {
        validate_credential_settings(&input.label, input.quota_threshold_percent)?;
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        validate_codex_group_and_proxy_transaction(&mut tx, group, input.proxy_id).await?;
        let row=sqlx::query_as::<_,CodexOauthFlowRecordRow>(
            "INSERT INTO codex_oauth_flows(id,actor_user_id,channel_group_id,label,proxy_id,quota_threshold_percent,
             redirect_uri,state_hash,code_verifier,expires_at) VALUES (?,?,?,?,?,?,?,?,?,?) RETURNING *")
            .bind(SqliteUuid(Uuid::new_v4())).bind(SqliteUuid(actor)).bind(SqliteUuid(group))
            .bind(input.label.trim()).bind(input.proxy_id.map(SqliteUuid)).bind(input.quota_threshold_percent)
            .bind(redirect_uri).bind(state_hash).bind(code_verifier).bind(SqliteTimestamp(expires_at))
            .fetch_one(&mut *tx).await?.0;
        tx.commit().await?;
        Ok(row)
    }
    pub async fn codex_oauth_flow(
        &self,
        id: Uuid,
        actor: Uuid,
    ) -> Result<Option<CodexOauthFlowRecord>, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        Ok(sqlx::query_as::<_,CodexOauthFlowRecordRow>(
            "SELECT * FROM codex_oauth_flows WHERE id=? AND actor_user_id=? AND completed_at IS NULL AND expires_at>ag_now()")
            .bind(SqliteUuid(id)).bind(SqliteUuid(actor)).fetch_optional(&mut *reader).await?.map(|r|r.0))
    }
    pub async fn export_codex_credentials(
        &self,
        group: Uuid,
        input: CodexCredentialExportInput,
    ) -> Result<CodexCredentialExportBundle, RepositoryError> {
        let ids = input
            .credential_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if ids.len() != input.credential_ids.len() || ids.len() > 1000 {
            return Err(RepositoryError::Validation);
        }
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        let mut tx = reader.begin().await?;
        let pool = codex_pool_context_connection(&mut tx, group).await?;
        let name: String = sqlx::query_scalar("SELECT name FROM channel_groups WHERE id=?")
            .bind(SqliteUuid(group))
            .fetch_one(&mut *tx)
            .await?;
        let rows=sqlx::query_as::<_,CodexCredentialRecordRow>(sqlx::AssertSqlSafe(credential_select(
            "WHERE c.connector_pool_id=?1 AND c.deleted_at IS NULL
             AND (?2 OR c.channel_id IN (SELECT value FROM json_each(?3))) ORDER BY c.label,c.channel_id")))
            .bind(SqliteUuid(pool.connector_pool_id)).bind(ids.is_empty()).bind(sqlx::types::Json(&ids))
            .fetch_all(&mut *tx).await?;
        if !ids.is_empty() && rows.len() != ids.len() {
            return Err(RepositoryError::NotFound);
        }
        let proxy_ids = rows
            .iter()
            .filter_map(|r| r.0.proxy_id)
            .collect::<BTreeSet<_>>();
        let mut proxies = Vec::new();
        if input.include_proxies && !proxy_ids.is_empty() {
            let rows = sqlx::query_as::<
                _,
                (
                    SqliteUuid,
                    String,
                    String,
                    Option<String>,
                    Option<String>,
                    sqlx::types::Json<Vec<String>>,
                    bool,
                ),
            >(
                "SELECT id,name,proxy_url,username,password,no_proxy_hosts,enabled FROM proxies
                 WHERE id IN (SELECT value FROM json_each(?)) ORDER BY name,id",
            )
            .bind(sqlx::types::Json(proxy_ids))
            .fetch_all(&mut *tx)
            .await?;
            for (id, name, url, username, password, no_proxy, enabled) in rows {
                proxies.push(CodexCredentialExportProxy {
                    proxy_key: id.0,
                    name,
                    proxy_url: url,
                    username,
                    password,
                    no_proxy_hosts: no_proxy.0,
                    enabled,
                });
            }
        }
        let credentials = rows
            .into_iter()
            .map(|r| {
                let r = r.0;
                CodexCredentialExportItem {
                    label: r.label,
                    email: r.email,
                    account_id: r.account_id,
                    user_id: r.user_id,
                    plan_type: r.plan_type,
                    is_fedramp: r.is_fedramp,
                    id_token: r.id_token,
                    access_token: r.access_token,
                    refresh_token: r.refresh_token,
                    proxy_key: r.proxy_id.filter(|_| input.include_proxies),
                    quota_threshold_percent: r.quota_threshold_percent,
                    enabled: r.enabled,
                }
            })
            .collect();
        tx.commit().await?;
        Ok(CodexCredentialExportBundle {
            export_type: "ai-gateway-codex-credentials",
            version: 2,
            exported_at: Utc::now(),
            channel_group_id: group,
            channel_group_name: name,
            proxies,
            credentials,
        })
    }
    pub async fn codex_credentials(
        &self,
        group: Uuid,
    ) -> Result<Vec<CodexCredentialView>, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        let mut tx = reader.begin().await?;
        let rows=sqlx::query_as::<_,CodexCredentialRecordRow>(sqlx::AssertSqlSafe(credential_select(
            "WHERE c.connector_pool_id=(SELECT connector_pool_id FROM channel_groups WHERE id=? AND connector_kind='codex_oauth')
             AND c.deleted_at IS NULL ORDER BY c.label,c.channel_id")))
            .bind(SqliteUuid(group)).fetch_all(&mut *tx).await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            result.push(view(&mut tx, row.0).await?);
        }
        tx.commit().await?;
        Ok(result)
    }
    pub async fn codex_credential_view(
        &self,
        id: Uuid,
    ) -> Result<Option<CodexCredentialView>, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        let mut tx = reader.begin().await?;
        let row = sqlx::query_as::<_, CodexCredentialRecordRow>(sqlx::AssertSqlSafe(
            credential_select("WHERE c.channel_id=? AND c.deleted_at IS NULL"),
        ))
        .bind(SqliteUuid(id))
        .fetch_optional(&mut *tx)
        .await?;
        let result = match row {
            Some(row) => Some(view(&mut tx, row.0).await?),
            None => None,
        };
        tx.commit().await?;
        Ok(result)
    }
    pub async fn codex_quota_window_history(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<CodexQuotaWindowHistory, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        let mut tx = reader.begin().await?;
        let result = history(&mut tx, id, limit).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn self_codex_quota_credentials(
        &self,
        user: Uuid,
    ) -> Result<Vec<SelfCodexQuotaCredentialView>, RepositoryError> {
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        let mut tx = reader.begin().await?;
        let rows =
            sqlx::query_as::<_, CodexCredentialRecordRow>(sqlx::AssertSqlSafe(credential_select(
                &format!("{VISIBLE} ORDER BY visibility.channel_group_id,c.channel_id"),
            )))
            .bind(SqliteUuid(user))
            .fetch_all(&mut *tx)
            .await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            result.push(self_view(view(&mut tx, row.0).await?));
        }
        tx.commit().await?;
        Ok(result)
    }
    pub async fn self_codex_quota_window_history(
        &self,
        user: Uuid,
        id: Uuid,
        limit: i64,
    ) -> Result<SelfCodexQuotaWindowHistory, RepositoryError> {
        if !(1..=500).contains(&limit) {
            return Err(RepositoryError::Validation);
        }
        let mut reader = self.database.acquire_read().await.map_err(open_failure)?;
        let mut tx = reader.begin().await?;
        let r = sqlx::query_as::<_, CodexCredentialRecordRow>(sqlx::AssertSqlSafe(
            credential_select(&format!("{VISIBLE} AND c.channel_id=?2")),
        ))
        .bind(SqliteUuid(user))
        .bind(SqliteUuid(id))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RepositoryError::NotFound)?
        .0;
        let periods = history(&mut tx, id, limit)
            .await?
            .periods
            .into_iter()
            .map(|r| SelfCodexQuotaWindowPeriodView {
                window_kind: r.window_kind,
                window_seconds: r.window_seconds,
                started_at: r.started_at,
                scheduled_reset_at: r.scheduled_reset_at,
                ended_at: r.ended_at,
                reset_reason: r.reset_reason,
                initial_used_percent: r.initial_used_percent,
                last_used_percent: r.last_used_percent,
                first_observed_at: r.first_observed_at,
                last_observed_at: r.last_observed_at,
                cost_amount: r.cost_amount,
            })
            .collect();
        tx.commit().await?;
        Ok(SelfCodexQuotaWindowHistory {
            credential_id: id,
            name: id.to_string(),
            channel_group_id: r.channel_group_id,
            plan_type: r.plan_type,
            periods,
        })
    }
    pub async fn mark_codex_credential_error(
        &self,
        id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        self.mark_codex_credential_error_transaction(&mut tx, id, permanent, code, summary)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn cleanup_codex_oauth_flows(&self) -> Result<u64, RepositoryError> {
        let mut tx = self.database.begin_write().await.map_err(open_failure)?;
        let result = sqlx::query(
            "DELETE FROM codex_oauth_flows WHERE expires_at<ag_now() OR completed_at IS NOT NULL",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(result)
    }
}

impl SqliteControlPlaneRepository {
    pub async fn persist_codex_quota(
        &self,
        channel_id: Uuid,
        quota: CodexQuotaUpdate,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let locked = sqlx::query_scalar::<_, SqliteUuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE channel_id=?1 AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(channel_id))
        .fetch_optional(&mut *transaction)
        .await?;
        if locked.is_none() {
            return Err(RepositoryError::NotFound);
        }
        reconcile_codex_quota_windows(&mut transaction, channel_id, &quota).await?;
        sqlx::query(
            "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
             runtime_status=CASE \
                 WHEN NOT enabled THEN 'disabled' \
                 WHEN reauth_required THEN 'unavailable' \
                 WHEN NOT ?2 OR ?3 THEN 'unavailable' \
                 WHEN MAX(COALESCE(?4,0),COALESCE(?7,0)) >= quota_threshold_percent \
                     THEN 'draining' \
                 ELSE 'active' END, \
             quota_allowed=?2,quota_limit_reached=?3, \
             primary_used_percent=?4,primary_window_seconds=?5,primary_reset_at=?6, \
             secondary_used_percent=?7,secondary_window_seconds=?8,secondary_reset_at=?9, \
             quota_checked_at=?10,quota_reset_credits_available=?11, \
             last_error_code=CASE WHEN reauth_required THEN last_error_code ELSE NULL END, \
             last_error_summary=CASE WHEN reauth_required THEN last_error_summary ELSE NULL END \
             WHERE channel_id=?1 AND deleted_at IS NULL \
               AND (quota_checked_at IS NULL OR quota_checked_at <= ?10)",
        )
        .bind(SqliteUuid(channel_id))
        .bind(quota.allowed)
        .bind(quota.limit_reached)
        .bind(quota.primary_used_percent)
        .bind(quota.primary_window_seconds)
        .bind(quota.primary_reset_at.map(SqliteTimestamp))
        .bind(quota.secondary_used_percent)
        .bind(quota.secondary_window_seconds)
        .bind(quota.secondary_reset_at.map(SqliteTimestamp))
        .bind(SqliteTimestamp(quota.checked_at))
        .bind(quota.reset_credits_available)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }
    pub async fn record_codex_quota_reset(
        &self,
        actor_user_id: Uuid,
        channel_id: Uuid,
        event_id: Uuid,
        requested_at: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows_reset: i32,
    ) -> Result<Uuid, RepositoryError> {
        if !(0..=2).contains(&windows_reset) {
            return Err(RepositoryError::Validation);
        }
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let reset_credits_available = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT quota_reset_credits_available \
             FROM codex_oauth_credentials \
             WHERE channel_id=?1 AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(channel_id))
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        let correlation_id = self
            .record_codex_quota_reset_transaction(
                &mut transaction,
                actor_user_id,
                channel_id,
                event_id,
                requested_at,
                outcome,
                windows_reset,
                reset_credits_available,
            )
            .await?;
        transaction.commit().await?;
        Ok(correlation_id)
    }
}
