//! SQLite reader for coherent runtime-configuration snapshots.

use sqlx::{Sqlite, SqlitePool, Transaction};

use super::super::{
    error::RepositoryError,
    records::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        FORWARDING_SETTINGS_KEY, McpServerRecord, ModelRecord, ModelRuleRecord, ProxyRecord,
        RuntimeConfigRecords, SystemSettingsRecord,
    },
};

/// Feature-gated SQLite implementation of the runtime-snapshot read path.
///
/// Runtime URL selection remains disabled until the remaining repositories
/// are ported. This type accepts an already configured SQLite pool so tests and
/// later dispatch code can exercise the same immutable snapshot contract.
#[derive(Clone)]
pub struct SqliteRuntimeConfigRepository {
    pool: SqlitePool,
}

impl SqliteRuntimeConfigRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Loads control-plane records in one SQLite read transaction.
    pub async fn load(&self) -> Result<ControlPlaneRecords, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let records = Self::load_transaction(&mut transaction).await?;
        transaction.commit().await?;
        Ok(records)
    }

    /// Loads every record needed to compile one coherent runtime snapshot.
    pub async fn load_runtime(&self) -> Result<RuntimeConfigRecords, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let records = RuntimeConfigRecords {
            control_plane: Self::load_transaction(&mut transaction).await?,
            system_settings: Self::load_system_settings_transaction(&mut transaction).await?,
        };
        transaction.commit().await?;
        Ok(records)
    }

    async fn load_system_settings_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<SystemSettingsRecord, RepositoryError> {
        sqlx::query_as::<_, SystemSettingsRecord>(
            "SELECT setting_key,value,updated_at
             FROM system_settings
             WHERE setting_key=?1",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)
    }

    async fn load_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<ControlPlaneRecords, RepositoryError> {
        let api_keys = sqlx::query_as::<_, ApiKeyRecord>(
            "SELECT k.id,k.user_id,u.status AS user_status,
                    u.websocket_enabled AS user_websocket_enabled,
                    g.filter_fast_mode AS user_filter_fast_mode,
                    k.secret_value,k.status,k.expires_at,k.allowed_api_formats,k.permissions,
                    k.allowed_group_ids,k.allowed_channel_ids,k.requests_per_minute,
                    k.max_concurrent_requests,k.quota_limit_amount,k.quota_used_amount
             FROM api_keys AS k
             JOIN users AS u ON u.id=k.user_id
             JOIN user_groups AS g ON g.id=u.user_group_id
             WHERE NOT k.is_system
             ORDER BY k.id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let models = sqlx::query_as::<_, ModelRecord>(
            "SELECT id,source_model_id,currency,price_unit_tokens,price_effective_at,
                    input_unit_price,cached_input_unit_price,cache_write_unit_price,
                    output_unit_price,advanced_billing
             FROM models
             ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let model_rules = sqlx::query_as::<_, ModelRuleRecord>(
            "SELECT r.id,r.client_model,r.api_format,r.upstream_model_id,
                    m.enabled AS upstream_model_enabled,
                    m.currency AS upstream_model_currency,
                    m.price_unit_tokens,m.price_effective_at,m.input_unit_price,
                    m.cached_input_unit_price,m.cache_write_unit_price,m.output_unit_price,
                    m.advanced_billing,m.source_model_id AS upstream_model,
                    r.channel_group_ids,r.channel_ids,r.enabled
             FROM model_rules AS r
             JOIN models AS m ON m.id=r.upstream_model_id
             ORDER BY r.id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let groups = sqlx::query_as::<_, ChannelGroupRecord>(
            "SELECT id,name,api_format,connector_kind,request_compression,priority,
                    selection_strategy,enabled
             FROM channel_groups
             ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let channels = sqlx::query_as::<_, ChannelRecord>(
            "SELECT id,channel_group_id,api_format,name,base_url,enabled,supports_websocket,
                    supports_standalone_web_search,auto_disabled,auto_disable_allowed,weight,
                    billing_multiplier,proxy_id,config_template_id,override_document,
                    connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,
                    upstream_auth_kind,upstream_auth_header_name,upstream_api_key,
                    available_models,test_model
             FROM channels
             ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let proxies = sqlx::query_as::<_, ProxyRecord>(
            "SELECT id,name,proxy_url,username,password,no_proxy_hosts,enabled
             FROM proxies
             ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let templates = sqlx::query_as::<_, ConfigTemplateRecord>(
            "SELECT id,name,description,document,enabled
             FROM config_templates
             ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        let mcp_servers = sqlx::query_as::<_, McpServerRecord>(
            "SELECT id,slug,kind,name,description,model_rule_id,settings_version,settings,enabled
             FROM mcp_servers
             WHERE deleted_at IS NULL
             ORDER BY slug,id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        Ok(ControlPlaneRecords {
            api_keys,
            models,
            model_rules,
            groups,
            channels,
            proxies,
            templates,
            mcp_servers,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use sqlx::{
        sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
        types::Json,
    };

    use super::SqliteRuntimeConfigRepository;
    use crate::persistence::{FORWARDING_SETTINGS_KEY, SQLITE_MIGRATOR};

    #[tokio::test]
    async fn runtime_reads_keep_one_wal_snapshot_across_all_record_queries() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(directory.path().join("runtime-snapshot.sqlite3"))
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        SQLITE_MIGRATOR.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO system_settings (setting_key,value) VALUES (?1,?2)")
            .bind(FORWARDING_SETTINGS_KEY)
            .bind(Json(json!({"snapshot": "before"})))
            .execute(&pool)
            .await
            .unwrap();

        let mut transaction = pool.begin().await.unwrap();
        SqliteRuntimeConfigRepository::load_transaction(&mut transaction)
            .await
            .unwrap();

        sqlx::query("UPDATE system_settings SET value=?2 WHERE setting_key=?1")
            .bind(FORWARDING_SETTINGS_KEY)
            .bind(Json(json!({"snapshot": "after"})))
            .execute(&pool)
            .await
            .unwrap();

        let record =
            SqliteRuntimeConfigRepository::load_system_settings_transaction(&mut transaction)
                .await
                .unwrap();
        assert_eq!(record.value, json!({"snapshot": "before"}));
        transaction.commit().await.unwrap();

        let current = sqlx::query_scalar::<_, Json<serde_json::Value>>(
            "SELECT value FROM system_settings WHERE setting_key=?1",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(current.0, json!({"snapshot": "after"}));
    }
}
