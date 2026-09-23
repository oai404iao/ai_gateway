//! Sharing configuration and bounded background reconciliation queries.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::codex_sharing::{SharingGroup, SharingGroupInput, SharingRecord, SharingWindow};
use crate::persistence::*;

const GROUP_JSON: &str = "jsonb_build_object('id',s.id,\
     'channel_id',s.channel_id,'bound_credential_id',s.credential_id,'name',s.name,'enabled',s.enabled,'seats',s.seats,\
     'primary_limit_amount',s.primary_limit_amount::text,\
     'secondary_limit_amount',s.secondary_limit_amount::text,\
     'request_reservation_amount',s.request_reservation_amount::text,\
     'user_requests_per_minute',s.user_requests_per_minute,\
     'group_requests_per_minute',s.group_requests_per_minute,\
     'user_max_concurrent_requests',s.user_max_concurrent_requests,\
     'group_max_concurrent_requests',s.group_max_concurrent_requests,\
     'updated_at',s.updated_at)";

impl PostgresControlPlaneRepository {
    pub(super) async fn load_sharing_only_channels(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<Vec<Uuid>, RepositoryError> {
        // Protect every capability bound to a sharing-only credential, plus
        // capabilities reachable through another credential for the same
        // provider identity. No token material enters the compiled registry.
        Ok(sqlx::query_scalar(
            "WITH restricted AS (SELECT DISTINCT channel.credential_id,identity.user_id, \
                 COALESCE(identity.account_id,'') AS account_id \
             FROM upstream_channels channel \
             JOIN codex_oauth_credentials identity ON identity.channel_id=channel.credential_id \
             WHERE channel.sharing_only AND channel.deleted_at IS NULL AND channel.credential_id IS NOT NULL \
               AND identity.deleted_at IS NULL), \
             protected AS ( \
                 SELECT credential_id FROM restricted \
                 UNION \
                 SELECT alias.channel_id FROM restricted source \
                 JOIN codex_oauth_credentials alias ON source.user_id=alias.user_id \
                     AND COALESCE(alias.account_id,'')=source.account_id \
                 WHERE source.user_id IS NOT NULL AND alias.deleted_at IS NULL) \
             SELECT DISTINCT capability.id FROM channel_capabilities capability \
             JOIN upstream_channels channel ON channel.id=capability.channel_id \
             WHERE channel.deleted_at IS NULL AND capability.deleted_at IS NULL \
               AND channel.credential_id IN (SELECT credential_id FROM protected)",
        )
        .fetch_all(&mut **transaction)
        .await?)
    }

    pub async fn claim_sharing_ledger(
        &self,
        ledger_id: Uuid,
    ) -> Result<sqlx::PgConnection, RepositoryError> {
        let mut connection = self.pool.acquire().await?.detach();
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(23441967553678919)")
            .fetch_one(&mut connection)
            .await?;
        if !locked {
            return Err(RepositoryError::Conflict);
        }
        sqlx::query(
            "INSERT INTO codex_sharing_ledger (ledger_id) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(ledger_id)
        .execute(&mut connection)
        .await?;
        let persisted: Uuid = sqlx::query_scalar("SELECT ledger_id FROM codex_sharing_ledger")
            .fetch_one(&mut connection)
            .await?;
        if persisted != ledger_id {
            return Err(RepositoryError::Conflict);
        }
        Ok(connection)
    }

    pub async fn sharing_groups(
        &self,
        user: Option<Uuid>,
    ) -> Result<Vec<SharingGroup>, RepositoryError> {
        let values = sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(format!(
            "SELECT {GROUP_JSON} FROM codex_sharing_groups s \
             WHERE $1::uuid IS NULL OR (s.seats @> jsonb_build_array($1::uuid) AND EXISTS \
             (SELECT 1 FROM users u WHERE u.id=$1 AND u.status='active' \
              AND u.deleted_at IS NULL AND NOT u.is_system)) ORDER BY s.id"
        )))
        .bind(user)
        .fetch_all(&self.pool)
        .await?;
        values
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|_| RepositoryError::Validation))
            .collect()
    }
}

impl super::postgres_control_plane::PostgresMeteringQueries {
    pub async fn sharing_completed_costs(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<(Uuid, rust_decimal::Decimal)>, RepositoryError> {
        if ids.len() > 1000 {
            return Err(RepositoryError::Validation);
        }
        Ok(sqlx::query_as(
            "SELECT id,cost_amount FROM request_metering_facts WHERE id=ANY($1) AND amount_state IN ('priced','zero_by_policy')",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?)
    }
}

impl PostgresControlPlaneRepository {
    pub(super) async fn load_sharing_transaction(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<Vec<SharingRecord>, RepositoryError> {
        let windows: Vec<SharingWindow> = sqlx::query_as(
            "WITH observed AS (\
             SELECT p.id,p.credential_id,p.window_kind,p.scheduled_reset_at,p.last_used_percent AS used_percent,\
                    c.quota_checked_at AS checked_at,\
                    (c.primary_window_seconds IS NOT NULL)::int + \
                    (c.secondary_window_seconds IS NOT NULL)::int AS expected_count,\
                    count(*) OVER (PARTITION BY p.credential_id) AS observed_count \
             FROM codex_quota_window_periods p \
             JOIN codex_oauth_credentials c ON c.channel_id=p.credential_id \
             JOIN codex_sharing_groups s ON s.credential_id=p.credential_id \
             WHERE p.ended_at IS NULL AND c.deleted_at IS NULL AND c.enabled \
               AND p.last_observed_at=c.quota_checked_at \
               AND num_nonnulls(c.primary_used_percent,c.primary_window_seconds,c.primary_reset_at) IN (0,3) \
               AND num_nonnulls(c.secondary_used_percent,c.secondary_window_seconds,c.secondary_reset_at) IN (0,3) \
               AND ((p.window_kind='primary' AND p.scheduled_reset_at=c.primary_reset_at \
                     AND p.window_seconds=c.primary_window_seconds AND p.last_used_percent=c.primary_used_percent) \
                 OR (p.window_kind='secondary' AND p.scheduled_reset_at=c.secondary_reset_at \
                     AND p.window_seconds=c.secondary_window_seconds AND p.last_used_percent=c.secondary_used_percent))\
             ) SELECT id,credential_id,window_kind,scheduled_reset_at,used_percent,checked_at \
               FROM observed WHERE observed_count=expected_count",
        ).fetch_all(&mut **transaction).await?;
        let rows =
            sqlx::query_as::<_, (Value, Vec<Uuid>, Vec<Uuid>)>(sqlx::AssertSqlSafe(format!(
                "SELECT {GROUP_JSON},\
             ARRAY(SELECT capability.id FROM upstream_channels channel \
                   JOIN channel_capabilities capability ON capability.channel_id=channel.id \
                   WHERE channel.deleted_at IS NULL AND capability.deleted_at IS NULL \
                     AND channel.id=s.channel_id AND channel.credential_id=s.credential_id),\
             ARRAY(SELECT capability.id FROM upstream_channels channel \
                   JOIN channel_capabilities capability ON capability.channel_id=channel.id \
                   JOIN codex_oauth_credentials identity ON identity.channel_id=channel.credential_id \
                   WHERE channel.deleted_at IS NULL AND capability.deleted_at IS NULL \
                     AND (identity.channel_id=s.credential_id OR \
                       (COALESCE(identity.account_id,'')=s.provider_account_id \
                        AND identity.user_id=s.provider_user_id))) \
             FROM codex_sharing_groups s ORDER BY s.id"
            )))
            .fetch_all(&mut **transaction)
            .await?;
        rows.into_iter()
            .map(|(value, channel_ids, protected_channel_ids)| {
                let group: SharingGroup =
                    serde_json::from_value(value).map_err(|_| RepositoryError::Validation)?;
                let windows = windows
                    .iter()
                    .filter(|w| w.credential_id == group.bound_credential_id)
                    .cloned()
                    .collect();
                Ok(SharingRecord {
                    group,
                    channel_ids,
                    protected_channel_ids,
                    windows,
                })
            })
            .collect()
    }
}

pub(super) async fn save_group(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    mut input: SharingGroupInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if !input.valid() {
        return Err(RepositoryError::Validation);
    }
    input.name = input.name.trim().to_owned();
    let before = sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(format!(
        "SELECT {GROUP_JSON} FROM codex_sharing_groups s WHERE id=$1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    match (&before, expected) {
        (Some(value), Some(version)) => {
            let previous: SharingGroup =
                serde_json::from_value(value.clone()).map_err(|_| RepositoryError::Validation)?;
            if previous.updated_at != version {
                return Err(RepositoryError::Conflict);
            }
            if previous.policy.channel_id != input.channel_id
                || input.seats.len() < previous.policy.seats.len()
            {
                return Err(RepositoryError::Validation);
            }
        }
        (None, None) => {}
        (None, Some(_)) => return Err(RepositoryError::NotFound),
        _ => return Err(RepositoryError::Conflict),
    }
    let previous_seats: Vec<Option<Uuid>> = before
        .as_ref()
        .and_then(|value| value.get("seats"))
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| RepositoryError::Validation)?
        .unwrap_or_default();
    let members = input
        .seats
        .iter()
        .enumerate()
        .filter(|(index, user)| previous_seats.get(*index) != Some(*user))
        .filter_map(|(_, user)| *user)
        .collect::<Vec<_>>();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE id=ANY($1) \
         AND status='active' AND deleted_at IS NULL AND NOT is_system",
    )
    .bind(&members)
    .fetch_one(&mut **transaction)
    .await?;
    if count != members.len() as i64 {
        return Err(RepositoryError::Validation);
    }
    let identity = sqlx::query_as::<_, (String, String, Uuid)>(
        "SELECT COALESCE(credential.account_id,''),credential.user_id,credential.channel_id
         FROM upstream_channels channel
         JOIN upstream_accesses access ON access.id=channel.access_id AND access.connector_kind='codex'
         JOIN codex_oauth_credentials credential ON credential.channel_id=channel.credential_id
         WHERE channel.id=$1 AND channel.deleted_at IS NULL AND access.deleted_at IS NULL
           AND credential.deleted_at IS NULL AND credential.user_id IS NOT NULL AND credential.user_id<>''
         FOR SHARE OF channel,credential",
    )
    .bind(input.channel_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Validation)?;
    let seated_users = input
        .seats
        .iter()
        .flatten()
        .map(Uuid::to_string)
        .collect::<Vec<_>>();
    let conflict: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM codex_sharing_groups WHERE id<>$1 \
         AND (credential_id=$2 \
              OR (provider_account_id=$3 AND provider_user_id=$4) \
              OR EXISTS (SELECT 1 FROM jsonb_array_elements_text(seats) member(user_id) \
                         WHERE member.user_id=ANY($5))))",
    )
    .bind(id)
    .bind(identity.2)
    .bind(&identity.0)
    .bind(&identity.1)
    .bind(&seated_users)
    .fetch_one(&mut **transaction)
    .await?;
    if conflict {
        return Err(RepositoryError::Conflict);
    }
    let updated_at = sqlx::query_scalar(
        "INSERT INTO codex_sharing_groups \
         (id,credential_id,provider_account_id,provider_user_id,name,enabled,seats,\
          primary_limit_amount,secondary_limit_amount,request_reservation_amount,\
          user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests,\
          group_max_concurrent_requests,channel_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
         ON CONFLICT (id) DO UPDATE SET name=EXCLUDED.name,enabled=EXCLUDED.enabled,\
          seats=EXCLUDED.seats,primary_limit_amount=EXCLUDED.primary_limit_amount,\
          secondary_limit_amount=EXCLUDED.secondary_limit_amount,\
          request_reservation_amount=EXCLUDED.request_reservation_amount,\
          user_requests_per_minute=EXCLUDED.user_requests_per_minute,\
          group_requests_per_minute=EXCLUDED.group_requests_per_minute,\
          user_max_concurrent_requests=EXCLUDED.user_max_concurrent_requests,\
          group_max_concurrent_requests=EXCLUDED.group_max_concurrent_requests \
         RETURNING updated_at",
    )
    .bind(id)
    .bind(identity.2)
    .bind(identity.0)
    .bind(identity.1)
    .bind(input.name.trim())
    .bind(input.enabled)
    .bind(serde_json::to_value(&input.seats).map_err(|_| RepositoryError::Validation)?)
    .bind(input.primary_limit_amount)
    .bind(input.secondary_limit_amount)
    .bind(input.request_reservation_amount)
    .bind(input.user_requests_per_minute as i32)
    .bind(input.group_requests_per_minute as i32)
    .bind(input.user_max_concurrent_requests as i32)
    .bind(input.group_max_concurrent_requests as i32)
    .bind(input.channel_id)
    .fetch_one(&mut **transaction)
    .await?;
    let after = serde_json::to_value(SharingGroup {
        id,
        bound_credential_id: identity.2,
        policy: input,
        updated_at,
    })
    .map_err(|_| RepositoryError::Validation)?;
    Ok(MutationResult {
        object_type: "codex_sharing_group",
        id,
        action: if before.is_some() { "update" } else { "create" },
        before_redacted: before.unwrap_or_else(|| json!({})),
        after_redacted: after,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
