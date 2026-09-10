//! Sharing configuration and bounded background reconciliation queries.

use super::*;
use crate::domain::codex_sharing::{SharingGroup, SharingGroupInput, SharingRecord, SharingWindow};

const GROUP_JSON: &str = "jsonb_build_object('id',s.id,\
     'credential_id',s.credential_id,'name',s.name,'enabled',s.enabled,'seats',s.seats,\
     'primary_limit_amount',s.primary_limit_amount::text,\
     'secondary_limit_amount',s.secondary_limit_amount::text,\
     'request_reservation_amount',s.request_reservation_amount::text,\
     'user_requests_per_minute',s.user_requests_per_minute,\
     'group_requests_per_minute',s.group_requests_per_minute,\
     'user_max_concurrent_requests',s.user_max_concurrent_requests,\
     'group_max_concurrent_requests',s.group_max_concurrent_requests,\
     'updated_at',s.updated_at)";

impl ControlPlaneRepository {
    pub(super) async fn load_sharing_only_channels(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<Vec<Uuid>, RepositoryError> {
        // Protect identities across pools as well as both format projections.
        // No token/identity material enters the compiled registry.
        Ok(sqlx::query_scalar(
            "WITH restricted AS (SELECT DISTINCT source.channel_id,source.user_id,source.account_id \
             FROM codex_oauth_credentials source \
             JOIN codex_oauth_credential_channels source_projection ON source_projection.credential_id=source.channel_id \
             JOIN channels source_channel ON source_channel.id=source_projection.channel_id \
             JOIN channel_groups source_group ON source_group.id=source_channel.channel_group_id \
             WHERE source_group.sharing_only AND source.deleted_at IS NULL) \
             SELECT projection.channel_id FROM restricted \
             JOIN codex_oauth_credential_channels projection ON projection.credential_id=restricted.channel_id \
             UNION \
             SELECT projection.channel_id FROM restricted source \
             JOIN codex_oauth_credentials alias ON source.user_id=alias.user_id \
                 AND COALESCE(source.account_id,'')=COALESCE(alias.account_id,'') \
             JOIN codex_oauth_credential_channels projection ON projection.credential_id=alias.channel_id \
             WHERE source.user_id IS NOT NULL AND alias.deleted_at IS NULL"
        ).fetch_all(&mut **transaction).await?)
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
        let values = sqlx::query_scalar::<_, Value>(&format!(
            "SELECT {GROUP_JSON} FROM codex_sharing_groups s \
             WHERE $1::uuid IS NULL OR (s.seats @> jsonb_build_array($1::uuid) AND EXISTS \
             (SELECT 1 FROM users u WHERE u.id=$1 AND u.status='active' \
              AND u.deleted_at IS NULL AND NOT u.is_system)) ORDER BY s.id"
        ))
        .bind(user)
        .fetch_all(&self.pool)
        .await?;
        values
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|_| RepositoryError::Validation))
            .collect()
    }

    pub async fn sharing_completed_costs(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<(Uuid, rust_decimal::Decimal)>, RepositoryError> {
        if ids.len() > 1000 {
            return Err(RepositoryError::Validation);
        }
        Ok(sqlx::query_as(
            "SELECT id,cost_amount FROM request_logs WHERE id=ANY($1) AND cost_amount IS NOT NULL",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?)
    }

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
        let rows = sqlx::query_as::<_, (Value, Vec<Uuid>, Vec<Uuid>)>(&format!(
            "SELECT {GROUP_JSON},\
             ARRAY(SELECT p.channel_id FROM codex_oauth_credential_channels p \
                   WHERE p.credential_id=s.credential_id),\
             ARRAY(SELECT p.channel_id FROM codex_oauth_credentials c \
                   JOIN codex_oauth_credential_channels p ON p.credential_id=c.channel_id \
                   WHERE c.channel_id=s.credential_id OR \
                     (COALESCE(c.account_id,'')=s.provider_account_id \
                      AND c.user_id=s.provider_user_id)) \
             FROM codex_sharing_groups s ORDER BY s.id"
        ))
        .fetch_all(&mut **transaction)
        .await?;
        rows.into_iter()
            .map(|(value, channel_ids, protected_channel_ids)| {
                let group: SharingGroup =
                    serde_json::from_value(value).map_err(|_| RepositoryError::Validation)?;
                let windows = windows
                    .iter()
                    .filter(|w| w.credential_id == group.policy.credential_id)
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
    let before = sqlx::query_scalar::<_, Value>(&format!(
        "SELECT {GROUP_JSON} FROM codex_sharing_groups s WHERE id=$1 FOR UPDATE"
    ))
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
            if previous.policy.credential_id != input.credential_id
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
    let identity = sqlx::query_as::<_, (String, String)>(
        "SELECT COALESCE(account_id,''),user_id FROM codex_oauth_credentials \
         WHERE channel_id=$1 AND deleted_at IS NULL AND user_id IS NOT NULL AND user_id<>''",
    )
    .bind(input.credential_id)
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
    .bind(input.credential_id)
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
          group_max_concurrent_requests) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) \
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
    .bind(input.credential_id)
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
    .fetch_one(&mut **transaction)
    .await?;
    let after = serde_json::to_value(SharingGroup {
        id,
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
