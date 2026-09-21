//! Transaction-scoped snapshot loading without legacy topology or grant arrays.

use sqlx::PgConnection;

use super::runtime::{BaseControlPlaneRecords, resolve_runtime};
use crate::persistence::{ControlPlaneRecords, RepositoryError};

/// The caller must hold a consistent snapshot transaction across this load
/// and the loading of system settings and sharing state.
pub async fn pg_load_control_plane(
    connection: &mut PgConnection,
) -> Result<ControlPlaneRecords, RepositoryError> {
    let topology = super::pg_load(connection).await?;
    let base = BaseControlPlaneRecords {
        api_keys: super::pg_decode(
            connection,
            "SELECT row_to_json(record)::text FROM (
                SELECT k.id,k.user_id,u.status AS user_status,
                    u.websocket_enabled AS user_websocket_enabled,
                    g.filter_fast_mode AS user_filter_fast_mode,k.secret_value,k.status,k.expires_at,
                    k.allowed_api_formats::text[] AS allowed_api_formats,k.permissions,
                    ARRAY[]::uuid[] AS allowed_group_ids,ARRAY[]::uuid[] AS allowed_channel_ids,
                    k.requests_per_minute,k.max_concurrent_requests,
                    k.quota_limit_amount::text,k.quota_used_amount::text
                FROM api_keys k JOIN users u ON u.id=k.user_id AND u.deleted_at IS NULL
                JOIN user_groups g ON g.id=u.user_group_id AND g.deleted_at IS NULL
                WHERE NOT k.is_system AND k.deleted_at IS NULL ORDER BY k.id
            ) record",
        ).await?,
        models: super::pg_decode(
            connection,
            "SELECT row_to_json(record)::text FROM (
                SELECT id,source_model_id,currency,price_unit_tokens,price_effective_at,
                    input_unit_price::text,cached_input_unit_price::text,
                    cache_write_unit_price::text,output_unit_price::text,advanced_billing
                FROM models WHERE deleted_at IS NULL ORDER BY id
            ) record",
        ).await?,
        proxies: super::pg_decode(
            connection,
            "SELECT row_to_json(record)::text FROM (
                SELECT id,name,proxy_url,username,password,no_proxy_hosts,enabled
                FROM proxies ORDER BY id
            ) record",
        ).await?,
        templates: super::pg_decode(
            connection,
            "SELECT row_to_json(record)::text FROM (
                SELECT id,name,description,document,enabled FROM config_templates ORDER BY id
            ) record",
        ).await?,
    };
    let profiles = super::pg_decode(
        connection,
        "SELECT row_to_json(record)::text FROM (
            SELECT p.id,p.model_id,m.enabled AS model_enabled,m.deleted_at IS NOT NULL AS model_deleted
            FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
            ORDER BY p.id
        ) record",
    )
    .await?;
    let credentials = crate::persistence::upstream_credentials::pg_records(connection).await?;
    Ok(resolve_runtime(&topology, base, &profiles, &credentials)?)
}

#[cfg(feature = "sqlite-backend")]
/// The caller must hold a consistent snapshot transaction across this load
/// and the loading of system settings and sharing state.
pub async fn sqlite_load_control_plane(
    connection: &mut sqlx::SqliteConnection,
) -> Result<ControlPlaneRecords, RepositoryError> {
    let topology = super::sqlite_load(connection).await?;
    let base = BaseControlPlaneRecords {
        api_keys: super::sqlite_decode(
            connection,
            "SELECT json_object(
                'id',k.id,'user_id',k.user_id,'user_status',u.status,
                'user_websocket_enabled',json(CASE u.websocket_enabled WHEN 1 THEN 'true' ELSE 'false' END),
                'user_filter_fast_mode',json(CASE g.filter_fast_mode WHEN 1 THEN 'true' ELSE 'false' END),
                'secret_value',k.secret_value,'status',k.status,'expires_at',k.expires_at,
                'allowed_api_formats',json(k.allowed_api_formats),'permissions',json(k.permissions),
                'allowed_group_ids',json('[]'),'allowed_channel_ids',json('[]'),
                'requests_per_minute',k.requests_per_minute,'max_concurrent_requests',k.max_concurrent_requests,
                'quota_limit_amount',k.quota_limit_amount,'quota_used_amount',k.quota_used_amount)
             FROM api_keys k JOIN users u ON u.id=k.user_id AND u.deleted_at IS NULL
             JOIN user_groups g ON g.id=u.user_group_id AND g.deleted_at IS NULL
             WHERE k.is_system=0 AND k.deleted_at IS NULL ORDER BY k.id",
        ).await?,
        models: super::sqlite_decode(
            connection,
            "SELECT json_object(
                'id',id,'source_model_id',source_model_id,'currency',currency,
                'price_unit_tokens',price_unit_tokens,'price_effective_at',price_effective_at,
                'input_unit_price',input_unit_price,'cached_input_unit_price',cached_input_unit_price,
                'cache_write_unit_price',cache_write_unit_price,'output_unit_price',output_unit_price,
                'advanced_billing',json(advanced_billing))
             FROM models WHERE deleted_at IS NULL ORDER BY id",
        ).await?,
        proxies: super::sqlite_decode(
            connection,
            "SELECT json_object(
                'id',id,'name',name,'proxy_url',proxy_url,'username',username,'password',password,
                'no_proxy_hosts',json(no_proxy_hosts),'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END))
             FROM proxies ORDER BY id",
        ).await?,
        templates: super::sqlite_decode(
            connection,
            "SELECT json_object(
                'id',id,'name',name,'description',description,'document',json(document),
                'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END))
             FROM config_templates ORDER BY id",
        ).await?,
    };
    let profiles = super::sqlite_decode(
        connection,
        "SELECT json_object('id',p.id,'model_id',p.model_id,
            'model_deleted',json(CASE WHEN m.deleted_at IS NOT NULL THEN 'true' ELSE 'false' END),
            'model_enabled',json(CASE m.enabled WHEN 1 THEN 'true' ELSE 'false' END))
         FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
         ORDER BY p.id",
    )
    .await?;
    let credentials = crate::persistence::sqlite::credential_records(connection).await?;
    Ok(resolve_runtime(&topology, base, &profiles, &credentials)?)
}
