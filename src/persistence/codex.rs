use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction, postgres::PgConnection};
use uuid::Uuid;

use super::{ControlPlaneRepository, MutationResult, RepositoryError};

const CODEX_CONNECTOR_KIND: &str = "codex_oauth";
const CODEX_API_FORMAT: &str = "open_ai_responses";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexOauthStartInput {
    pub label: String,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default = "default_weight")]
    pub weight: i32,
    #[serde(default = "default_quota_threshold_percent")]
    pub quota_threshold_percent: i16,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialImportInput {
    pub label: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default = "default_weight")]
    pub weight: i32,
    #[serde(default = "default_quota_threshold_percent")]
    pub quota_threshold_percent: i16,
    #[serde(default)]
    pub id_token: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialExportInput {
    #[serde(default)]
    pub credential_ids: Vec<Uuid>,
    #[serde(default = "default_include_proxies")]
    pub include_proxies: bool,
}

#[derive(Clone, Serialize)]
pub struct CodexCredentialExportBundle {
    #[serde(rename = "type")]
    pub export_type: &'static str,
    pub version: u8,
    pub exported_at: DateTime<Utc>,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub proxies: Vec<CodexCredentialExportProxy>,
    pub credentials: Vec<CodexCredentialExportItem>,
}

#[derive(Clone, Serialize, FromRow)]
pub struct CodexCredentialExportProxy {
    pub proxy_key: Uuid,
    pub name: String,
    pub proxy_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Serialize)]
pub struct CodexCredentialExportItem {
    pub label: String,
    pub email: Option<String>,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub proxy_key: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
    pub enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialUpdateInput {
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
}

#[derive(Clone)]
pub struct CodexCredentialCreate {
    pub channel_group_id: Uuid,
    pub label: String,
    pub enabled: bool,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
    pub base_url: String,
    pub email: Option<String>,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub available_models: Vec<String>,
    pub quota: Option<CodexQuotaUpdate>,
}

#[derive(Clone, FromRow)]
pub struct CodexOauthFlowRecord {
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub channel_group_id: Uuid,
    pub label: String,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
    pub redirect_uri: String,
    pub state_hash: Vec<u8>,
    pub code_verifier: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, FromRow)]
pub struct CodexCredentialRecord {
    pub channel_id: Uuid,
    pub channel_group_id: Uuid,
    pub label: String,
    pub email: Option<String>,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub last_refreshed_at: DateTime<Utc>,
    pub refresh_generation: i64,
    pub reauth_required: bool,
    pub quota_threshold_percent: i16,
    pub runtime_status: String,
    pub quota_allowed: Option<bool>,
    pub quota_limit_reached: Option<bool>,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub quota_checked_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub last_error_summary: Option<String>,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub available_models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for CodexCredentialRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexCredentialRecord")
            .field("channel_id", &self.channel_id)
            .field("channel_group_id", &self.channel_group_id)
            .field("label", &self.label)
            .field("email", &self.email)
            .field("account_id", &self.account_id)
            .field("plan_type", &self.plan_type)
            .field("is_fedramp", &self.is_fedramp)
            .field("id_token", &"REDACTED")
            .field("access_token", &"REDACTED")
            .field("refresh_token", &"REDACTED")
            .field("access_token_expires_at", &self.access_token_expires_at)
            .field("last_refreshed_at", &self.last_refreshed_at)
            .field("refresh_generation", &self.refresh_generation)
            .field("reauth_required", &self.reauth_required)
            .field("quota_threshold_percent", &self.quota_threshold_percent)
            .field("runtime_status", &self.runtime_status)
            .field("quota_allowed", &self.quota_allowed)
            .field("quota_limit_reached", &self.quota_limit_reached)
            .field("primary_used_percent", &self.primary_used_percent)
            .field("secondary_used_percent", &self.secondary_used_percent)
            .field("quota_checked_at", &self.quota_checked_at)
            .field("last_error_code", &self.last_error_code)
            .field("last_error_summary", &self.last_error_summary)
            .field("proxy_id", &self.proxy_id)
            .field("weight", &self.weight)
            .field("enabled", &self.enabled)
            .field("available_models", &self.available_models)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct CodexCredentialView {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub label: String,
    pub email: Option<String>,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub last_refreshed_at: DateTime<Utc>,
    pub quota_threshold_percent: i16,
    pub runtime_status: String,
    pub quota_allowed: Option<bool>,
    pub quota_limit_reached: Option<bool>,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub quota_checked_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub last_error_summary: Option<String>,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub available_models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct CodexQuotaUpdate {
    pub allowed: bool,
    pub limit_reached: bool,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct CodexTokenRefreshUpdate {
    pub expected_generation: i64,
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: Option<bool>,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub refreshed_at: DateTime<Utc>,
}

impl ControlPlaneRepository {
    pub async fn begin_codex_refresh(&self) -> Result<Transaction<'_, Postgres>, RepositoryError> {
        self.pool.begin().await.map_err(RepositoryError::from)
    }

    pub async fn codex_credentials(
        &self,
        channel_group_id: Uuid,
    ) -> Result<Vec<CodexCredentialView>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialView>(
            "SELECT c.channel_id AS id,c.channel_group_id,c.label,c.email,c.account_id,c.plan_type, \
                    c.is_fedramp,c.access_token_expires_at,c.last_refreshed_at, \
                    c.quota_threshold_percent,c.runtime_status,c.quota_allowed, \
                    c.quota_limit_reached,c.primary_used_percent,c.primary_window_seconds, \
                    c.primary_reset_at,c.secondary_used_percent,c.secondary_window_seconds, \
                    c.secondary_reset_at,c.quota_checked_at,c.last_error_code, \
                    c.last_error_summary,ch.proxy_id,ch.weight,c.enabled,ch.available_models, \
                    c.created_at,c.updated_at \
             FROM codex_oauth_credentials c \
             JOIN channels ch ON ch.id=c.channel_id \
             WHERE c.channel_group_id=$1 \
             ORDER BY c.label,c.channel_id",
        )
        .bind(channel_group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_credential_view(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialView>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialView>(
            "SELECT c.channel_id AS id,c.channel_group_id,c.label,c.email,c.account_id,c.plan_type, \
                    c.is_fedramp,c.access_token_expires_at,c.last_refreshed_at, \
                    c.quota_threshold_percent,c.runtime_status,c.quota_allowed, \
                    c.quota_limit_reached,c.primary_used_percent,c.primary_window_seconds, \
                    c.primary_reset_at,c.secondary_used_percent,c.secondary_window_seconds, \
                    c.secondary_reset_at,c.quota_checked_at,c.last_error_code, \
                    c.last_error_summary,ch.proxy_id,ch.weight,c.enabled,ch.available_models, \
                    c.created_at,c.updated_at \
             FROM codex_oauth_credentials c \
             JOIN channels ch ON ch.id=c.channel_id \
             WHERE c.channel_id=$1",
        )
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_credential(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialRecord>(&credential_select("WHERE c.channel_id=$1"))
            .bind(channel_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::from)
    }

    pub async fn codex_credential_for_update(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
            "WHERE c.channel_id=$1 FOR UPDATE OF c,ch",
        ))
        .bind(channel_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn load_codex_credentials(
        &self,
    ) -> Result<Vec<CodexCredentialRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialRecord>(&credential_select("ORDER BY c.channel_id"))
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::from)
    }

    pub async fn export_codex_credentials(
        &self,
        channel_group_id: Uuid,
        input: CodexCredentialExportInput,
    ) -> Result<CodexCredentialExportBundle, RepositoryError> {
        const MAX_SELECTED_CREDENTIALS: usize = 1_000;

        if input.credential_ids.len() > MAX_SELECTED_CREDENTIALS {
            return Err(RepositoryError::Validation);
        }
        let selected_ids = input
            .credential_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if selected_ids.len() != input.credential_ids.len() {
            return Err(RepositoryError::Validation);
        }

        let channel_group_name = sqlx::query_scalar::<_, String>(
            "SELECT name FROM channel_groups \
             WHERE id=$1 AND connector_kind=$2 AND api_format=$3::api_format",
        )
        .bind(channel_group_id)
        .bind(CODEX_CONNECTOR_KIND)
        .bind(CODEX_API_FORMAT)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::NotFound)?;

        let records = if selected_ids.is_empty() {
            sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
                "WHERE c.channel_group_id=$1 ORDER BY c.label,c.channel_id",
            ))
            .bind(channel_group_id)
            .fetch_all(&self.pool)
            .await?
        } else {
            let selected_ids = selected_ids.into_iter().collect::<Vec<_>>();
            let records = sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
                "WHERE c.channel_group_id=$1 AND c.channel_id=ANY($2) \
                 ORDER BY c.label,c.channel_id",
            ))
            .bind(channel_group_id)
            .bind(&selected_ids)
            .fetch_all(&self.pool)
            .await?;
            if records.len() != selected_ids.len() {
                return Err(RepositoryError::NotFound);
            }
            records
        };

        let proxy_ids = records
            .iter()
            .filter_map(|record| record.proxy_id)
            .collect::<BTreeSet<_>>();
        let proxies = if input.include_proxies && !proxy_ids.is_empty() {
            let proxy_ids = proxy_ids.into_iter().collect::<Vec<_>>();
            sqlx::query_as::<_, CodexCredentialExportProxy>(
                "SELECT id AS proxy_key,name,proxy_url,username,password,no_proxy_hosts,enabled \
                 FROM proxies WHERE id=ANY($1) ORDER BY name,id",
            )
            .bind(&proxy_ids)
            .fetch_all(&self.pool)
            .await?
        } else {
            Vec::new()
        };
        let credentials = records
            .into_iter()
            .map(|record| CodexCredentialExportItem {
                label: record.label,
                email: record.email,
                account_id: record.account_id,
                plan_type: record.plan_type,
                is_fedramp: record.is_fedramp,
                id_token: record.id_token,
                access_token: record.access_token,
                refresh_token: record.refresh_token,
                proxy_key: record.proxy_id.filter(|_| input.include_proxies),
                weight: record.weight,
                quota_threshold_percent: record.quota_threshold_percent,
                enabled: record.enabled,
            })
            .collect();

        Ok(CodexCredentialExportBundle {
            export_type: "ai-gateway-codex-credentials",
            version: 1,
            exported_at: Utc::now(),
            channel_group_id,
            channel_group_name,
            proxies,
            credentials,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_codex_oauth_flow(
        &self,
        actor_user_id: Uuid,
        channel_group_id: Uuid,
        input: CodexOauthStartInput,
        redirect_uri: String,
        state_hash: Vec<u8>,
        code_verifier: String,
        expires_at: DateTime<Utc>,
    ) -> Result<CodexOauthFlowRecord, RepositoryError> {
        validate_credential_settings(&input.label, input.weight, input.quota_threshold_percent)?;
        validate_codex_group_and_proxy_pool(&self.pool, channel_group_id, input.proxy_id).await?;
        let id = Uuid::new_v4();
        sqlx::query_as::<_, CodexOauthFlowRecord>(
            "INSERT INTO codex_oauth_flows \
             (id,actor_user_id,channel_group_id,label,proxy_id,weight,quota_threshold_percent, \
              redirect_uri,state_hash,code_verifier,expires_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) \
             RETURNING id,actor_user_id,channel_group_id,label,proxy_id,weight, \
                       quota_threshold_percent,redirect_uri,state_hash,code_verifier,expires_at",
        )
        .bind(id)
        .bind(actor_user_id)
        .bind(channel_group_id)
        .bind(input.label.trim())
        .bind(input.proxy_id)
        .bind(input.weight)
        .bind(input.quota_threshold_percent)
        .bind(redirect_uri)
        .bind(state_hash)
        .bind(code_verifier)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_oauth_flow(
        &self,
        id: Uuid,
        actor_user_id: Uuid,
    ) -> Result<Option<CodexOauthFlowRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexOauthFlowRecord>(
            "SELECT id,actor_user_id,channel_group_id,label,proxy_id,weight, \
                    quota_threshold_percent,redirect_uri,state_hash,code_verifier,expires_at \
             FROM codex_oauth_flows \
             WHERE id=$1 AND actor_user_id=$2 AND completed_at IS NULL AND expires_at>now()",
        )
        .bind(id)
        .bind(actor_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn insert_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<MutationResult, RepositoryError> {
        validate_credential_settings(&input.label, input.weight, input.quota_threshold_percent)?;
        validate_codex_group_and_proxy_transaction(
            transaction,
            input.channel_group_id,
            input.proxy_id,
        )
        .await?;
        if input.account_id.trim().is_empty()
            || input.account_id.len() > 300
            || input.id_token.is_empty()
            || input.access_token.is_empty()
            || input.refresh_token.is_empty()
            || input.available_models.is_empty()
        {
            return Err(RepositoryError::Validation);
        }
        let existing_channel_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE channel_group_id=$1 AND account_id=$2 \
             FOR UPDATE",
        )
        .bind(input.channel_group_id)
        .bind(&input.account_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if let Some(flow_id) = oauth_flow_id {
            let updated = sqlx::query(
                "UPDATE codex_oauth_flows SET completed_at=now() \
                 WHERE id=$1 AND channel_group_id=$2 AND completed_at IS NULL AND expires_at>now()",
            )
            .bind(flow_id)
            .bind(input.channel_group_id)
            .execute(&mut **transaction)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(RepositoryError::Conflict);
            }
        }

        if let Some(channel_id) = existing_channel_id {
            let before = codex_credential_audit(transaction, channel_id).await?;
            sqlx::query(
                "UPDATE channels SET \
                 name=$2,base_url=$3,enabled=true,weight=$4,proxy_id=$5,available_models=$6 \
                 WHERE id=$1",
            )
            .bind(channel_id)
            .bind(input.label.trim())
            .bind(&input.base_url)
            .bind(input.weight)
            .bind(input.proxy_id)
            .bind(&input.available_models)
            .execute(&mut **transaction)
            .await?;

            let quota = input.quota.as_ref();
            let has_quota = quota.is_some();
            let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
                "UPDATE codex_oauth_credentials SET \
                 label=$2,email=$3,plan_type=$4,is_fedramp=$5,id_token=$6,access_token=$7, \
                 refresh_token=$8,access_token_expires_at=$9,last_refreshed_at=$10, \
                 refresh_generation=refresh_generation+1,reauth_required=false,enabled=$12, \
                 quota_threshold_percent=$11,runtime_status=CASE \
                     WHEN NOT $12 THEN 'disabled' \
                     WHEN $13 THEN CASE \
                         WHEN NOT $14 OR $15 THEN 'unavailable' \
                         WHEN GREATEST(COALESCE($16,0),COALESCE($19,0)) >= $11 \
                             THEN 'draining' \
                         ELSE 'active' END \
                     WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                     WHEN GREATEST(COALESCE(primary_used_percent,0), \
                                   COALESCE(secondary_used_percent,0)) >= $11 \
                         THEN 'draining' \
                     ELSE 'active' END, \
                 quota_allowed=CASE WHEN $13 THEN $14 ELSE quota_allowed END, \
                 quota_limit_reached=CASE WHEN $13 THEN $15 ELSE quota_limit_reached END, \
                 primary_used_percent=CASE WHEN $13 THEN $16 ELSE primary_used_percent END, \
                 primary_window_seconds=CASE WHEN $13 THEN $17 ELSE primary_window_seconds END, \
                 primary_reset_at=CASE WHEN $13 THEN $18 ELSE primary_reset_at END, \
                 secondary_used_percent=CASE WHEN $13 THEN $19 ELSE secondary_used_percent END, \
                 secondary_window_seconds=CASE WHEN $13 THEN $20 ELSE secondary_window_seconds END, \
                 secondary_reset_at=CASE WHEN $13 THEN $21 ELSE secondary_reset_at END, \
                 quota_checked_at=CASE WHEN $13 THEN $22 ELSE quota_checked_at END, \
                 last_error_code=NULL,last_error_summary=NULL \
                 WHERE channel_id=$1 \
                 RETURNING updated_at",
            )
            .bind(channel_id)
            .bind(input.label.trim())
            .bind(input.email)
            .bind(input.plan_type)
            .bind(input.is_fedramp)
            .bind(input.id_token)
            .bind(input.access_token)
            .bind(input.refresh_token)
            .bind(input.access_token_expires_at)
            .bind(Utc::now())
            .bind(input.quota_threshold_percent)
            .bind(input.enabled)
            .bind(has_quota)
            .bind(quota.map(|quota| quota.allowed))
            .bind(quota.map(|quota| quota.limit_reached))
            .bind(quota.and_then(|quota| quota.primary_used_percent))
            .bind(quota.and_then(|quota| quota.primary_window_seconds))
            .bind(quota.and_then(|quota| quota.primary_reset_at))
            .bind(quota.and_then(|quota| quota.secondary_used_percent))
            .bind(quota.and_then(|quota| quota.secondary_window_seconds))
            .bind(quota.and_then(|quota| quota.secondary_reset_at))
            .bind(quota.map(|quota| quota.checked_at))
            .fetch_one(&mut **transaction)
            .await?;

            return Ok(MutationResult {
                id: channel_id,
                object_type: "codex_oauth_credential",
                action: "update",
                before_redacted: before,
                after_redacted: codex_credential_audit(transaction, channel_id).await?,
                created_secret: None,
                reason: None,
                updated_at,
                correlation_id: None,
            });
        }

        let channel_id = Uuid::new_v4();
        let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            "INSERT INTO channels \
             (id,channel_group_id,api_format,name,base_url,enabled,weight,billing_multiplier, \
              proxy_id,override_document,upstream_auth_kind,available_models, \
              status_statistics_enabled,auto_disable_allowed,supports_websocket) \
             VALUES ($1,$2,$3::api_format,$4,$5,true,$6,1,$7,'{}','none',$8,false,false,false) \
             RETURNING updated_at",
        )
        .bind(channel_id)
        .bind(input.channel_group_id)
        .bind(CODEX_API_FORMAT)
        .bind(input.label.trim())
        .bind(input.base_url)
        .bind(input.weight)
        .bind(input.proxy_id)
        .bind(&input.available_models)
        .fetch_one(&mut **transaction)
        .await?;

        let quota = input.quota.as_ref();
        let runtime_status = if input.enabled {
            quota.map_or("active", |quota| {
                runtime_status_for_quota(quota, input.quota_threshold_percent)
            })
        } else {
            "disabled"
        };
        sqlx::query(
            "INSERT INTO codex_oauth_credentials \
             (channel_id,channel_group_id,label,email,account_id,plan_type,is_fedramp,id_token, \
              access_token,refresh_token,access_token_expires_at,last_refreshed_at, \
              enabled,quota_threshold_percent,runtime_status,quota_allowed,quota_limit_reached, \
              primary_used_percent,primary_window_seconds,primary_reset_at, \
              secondary_used_percent,secondary_window_seconds,secondary_reset_at,quota_checked_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24)",
        )
        .bind(channel_id)
        .bind(input.channel_group_id)
        .bind(input.label.trim())
        .bind(input.email)
        .bind(input.account_id)
        .bind(input.plan_type)
        .bind(input.is_fedramp)
        .bind(input.id_token)
        .bind(input.access_token)
        .bind(input.refresh_token)
        .bind(input.access_token_expires_at)
        .bind(Utc::now())
        .bind(input.enabled)
        .bind(input.quota_threshold_percent)
        .bind(runtime_status)
        .bind(quota.map(|quota| quota.allowed))
        .bind(quota.map(|quota| quota.limit_reached))
        .bind(quota.and_then(|quota| quota.primary_used_percent))
        .bind(quota.and_then(|quota| quota.primary_window_seconds))
        .bind(quota.and_then(|quota| quota.primary_reset_at))
        .bind(quota.and_then(|quota| quota.secondary_used_percent))
        .bind(quota.and_then(|quota| quota.secondary_window_seconds))
        .bind(quota.and_then(|quota| quota.secondary_reset_at))
        .bind(quota.map(|quota| quota.checked_at))
        .execute(&mut **transaction)
        .await?;

        Ok(MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "create",
            before_redacted: json!({}),
            after_redacted: codex_credential_audit(transaction, channel_id).await?,
            created_secret: None,
            reason: None,
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn update_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, RepositoryError> {
        validate_credential_settings(&input.label, input.weight, input.quota_threshold_percent)?;
        let before = codex_credential_audit(transaction, channel_id).await?;
        let group_id = before["channel_group_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(RepositoryError::Validation)?;
        validate_codex_group_and_proxy_transaction(transaction, group_id, input.proxy_id).await?;
        let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            "UPDATE codex_oauth_credentials \
             SET label=$2,quota_threshold_percent=$3,enabled=$4,runtime_status=CASE \
                 WHEN NOT $4 THEN 'disabled' \
                 WHEN reauth_required THEN 'unavailable' \
                 WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                 WHEN GREATEST(COALESCE(primary_used_percent,0), \
                               COALESCE(secondary_used_percent,0)) >= $3 \
                     THEN 'draining' \
                 ELSE 'active' END \
             WHERE channel_id=$1 AND updated_at=$5 \
             RETURNING updated_at",
        )
        .bind(channel_id)
        .bind(input.label.trim())
        .bind(input.quota_threshold_percent)
        .bind(input.enabled)
        .bind(expected_updated_at)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        sqlx::query("UPDATE channels SET name=$2,proxy_id=$3,weight=$4 WHERE id=$1")
            .bind(channel_id)
            .bind(input.label.trim())
            .bind(input.proxy_id)
            .bind(input.weight)
            .execute(&mut **transaction)
            .await?;

        Ok(MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "update",
            before_redacted: before,
            after_redacted: codex_credential_audit(transaction, channel_id).await?,
            created_secret: None,
            reason: None,
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn persist_codex_token_refresh_transaction(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        update: CodexTokenRefreshUpdate,
    ) -> Result<bool, RepositoryError> {
        let updated = sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             id_token=COALESCE($3,id_token),access_token=COALESCE($4,access_token), \
             refresh_token=COALESCE($5,refresh_token),email=COALESCE($6,email), \
             account_id=COALESCE($7,account_id),plan_type=COALESCE($8,plan_type), \
             is_fedramp=COALESCE($9,is_fedramp),access_token_expires_at=$10, \
             last_refreshed_at=$11,refresh_generation=refresh_generation+1, \
             reauth_required=false, \
             runtime_status=CASE \
                 WHEN NOT enabled THEN 'disabled' \
                 WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                 WHEN GREATEST(COALESCE(primary_used_percent,0), \
                               COALESCE(secondary_used_percent,0)) >= quota_threshold_percent \
                     THEN 'draining' \
                 ELSE 'active' END, \
             last_error_code=NULL,last_error_summary=NULL \
             WHERE channel_id=$1 AND refresh_generation=$2",
        )
        .bind(channel_id)
        .bind(update.expected_generation)
        .bind(update.id_token)
        .bind(update.access_token)
        .bind(update.refresh_token)
        .bind(update.email)
        .bind(update.account_id)
        .bind(update.plan_type)
        .bind(update.is_fedramp)
        .bind(update.access_token_expires_at)
        .bind(update.refreshed_at)
        .execute(&mut **transaction)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn persist_codex_quota(
        &self,
        channel_id: Uuid,
        quota: CodexQuotaUpdate,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             runtime_status=CASE \
                 WHEN NOT enabled THEN 'disabled' \
                 WHEN reauth_required THEN 'unavailable' \
                 WHEN NOT $2 OR $3 THEN 'unavailable' \
                 WHEN GREATEST(COALESCE($4,0),COALESCE($7,0)) >= quota_threshold_percent \
                     THEN 'draining' \
                 ELSE 'active' END, \
             quota_allowed=$2,quota_limit_reached=$3, \
             primary_used_percent=$4,primary_window_seconds=$5,primary_reset_at=$6, \
             secondary_used_percent=$7,secondary_window_seconds=$8,secondary_reset_at=$9, \
             quota_checked_at=$10, \
             last_error_code=CASE WHEN reauth_required THEN last_error_code ELSE NULL END, \
             last_error_summary=CASE WHEN reauth_required THEN last_error_summary ELSE NULL END \
             WHERE channel_id=$1",
        )
        .bind(channel_id)
        .bind(quota.allowed)
        .bind(quota.limit_reached)
        .bind(quota.primary_used_percent)
        .bind(quota.primary_window_seconds)
        .bind(quota.primary_reset_at)
        .bind(quota.secondary_used_percent)
        .bind(quota.secondary_window_seconds)
        .bind(quota.secondary_reset_at)
        .bind(quota.checked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_codex_credential_error(
        &self,
        channel_id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             reauth_required=reauth_required OR $2, \
             runtime_status=CASE WHEN $2 THEN 'unavailable' ELSE runtime_status END, \
             last_error_code=CASE WHEN reauth_required AND NOT $2 THEN last_error_code ELSE $3 END, \
             last_error_summary=CASE WHEN reauth_required AND NOT $2 THEN last_error_summary ELSE $4 END \
             WHERE channel_id=$1",
        )
        .bind(channel_id)
        .bind(permanent)
        .bind(code)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_codex_credential_error_transaction(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             reauth_required=reauth_required OR $2, \
             runtime_status=CASE WHEN $2 THEN 'unavailable' ELSE runtime_status END, \
             last_error_code=CASE WHEN reauth_required AND NOT $2 THEN last_error_code ELSE $3 END, \
             last_error_summary=CASE WHEN reauth_required AND NOT $2 THEN last_error_summary ELSE $4 END \
             WHERE channel_id=$1",
        )
        .bind(channel_id)
        .bind(permanent)
        .bind(code)
        .bind(summary)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    pub async fn cleanup_codex_oauth_flows(&self) -> Result<u64, RepositoryError> {
        let deleted = sqlx::query(
            "DELETE FROM codex_oauth_flows \
             WHERE expires_at < now() OR completed_at IS NOT NULL",
        )
        .execute(&self.pool)
        .await?;
        Ok(deleted.rows_affected())
    }
}

fn credential_select(suffix: &str) -> String {
    format!(
        "SELECT c.channel_id,c.channel_group_id,c.label,c.email,c.account_id,c.plan_type, \
                c.is_fedramp,c.id_token,c.access_token,c.refresh_token, \
                c.access_token_expires_at,c.last_refreshed_at,c.refresh_generation, \
                c.reauth_required, \
                c.quota_threshold_percent,c.runtime_status,c.quota_allowed, \
                c.quota_limit_reached,c.primary_used_percent,c.primary_window_seconds, \
                c.primary_reset_at,c.secondary_used_percent,c.secondary_window_seconds, \
                c.secondary_reset_at,c.quota_checked_at,c.last_error_code,c.last_error_summary, \
                ch.proxy_id,ch.weight,c.enabled,ch.available_models,c.created_at,c.updated_at \
         FROM codex_oauth_credentials c JOIN channels ch ON ch.id=c.channel_id {suffix}"
    )
}

async fn validate_codex_group_and_proxy_pool(
    pool: &PgPool,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    let mut connection = pool.acquire().await?;
    validate_codex_group_and_proxy_connection(&mut connection, channel_group_id, proxy_id).await
}

async fn validate_codex_group_and_proxy_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    validate_codex_group_and_proxy_connection(transaction, channel_group_id, proxy_id).await
}

async fn validate_codex_group_and_proxy_connection(
    connection: &mut PgConnection,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    let valid_group = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS( \
             SELECT 1 FROM channel_groups \
             WHERE id=$1 AND connector_kind=$2 AND api_format=$3::api_format \
         )",
    )
    .bind(channel_group_id)
    .bind(CODEX_CONNECTOR_KIND)
    .bind(CODEX_API_FORMAT)
    .fetch_one(&mut *connection)
    .await?;
    if !valid_group {
        return Err(RepositoryError::Validation);
    }
    if let Some(proxy_id) = proxy_id {
        let valid_proxy = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM proxies WHERE id=$1 AND enabled)",
        )
        .bind(proxy_id)
        .fetch_one(&mut *connection)
        .await?;
        if !valid_proxy {
            return Err(RepositoryError::Validation);
        }
    }
    Ok(())
}

async fn codex_credential_audit(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
) -> Result<Value, RepositoryError> {
    sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object( \
             'id',c.channel_id,'channel_group_id',c.channel_group_id,'label',c.label, \
             'email',c.email,'account_id',c.account_id,'plan_type',c.plan_type, \
             'is_fedramp',c.is_fedramp,'access_token_expires_at',c.access_token_expires_at, \
             'last_refreshed_at',c.last_refreshed_at, \
             'quota_threshold_percent',c.quota_threshold_percent, \
             'runtime_status',c.runtime_status,'proxy_id',ch.proxy_id,'weight',ch.weight, \
             'enabled',c.enabled,'available_models',ch.available_models, \
             'created_at',c.created_at,'updated_at',c.updated_at) \
         FROM codex_oauth_credentials c JOIN channels ch ON ch.id=c.channel_id \
         WHERE c.channel_id=$1 FOR UPDATE OF c,ch",
    )
    .bind(channel_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)
}

fn validate_credential_settings(
    label: &str,
    weight: i32,
    quota_threshold_percent: i16,
) -> Result<(), RepositoryError> {
    if label.trim().is_empty()
        || label.len() > 100
        || weight <= 0
        || !(1..=100).contains(&quota_threshold_percent)
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

const fn default_weight() -> i32 {
    100
}

const fn default_quota_threshold_percent() -> i16 {
    95
}

const fn default_include_proxies() -> bool {
    true
}

const fn default_enabled() -> bool {
    true
}
