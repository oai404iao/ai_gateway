//! SQLite schema and migration foundation.
//!
//! Runtime repository dispatch is intentionally not exposed yet. This module
//! owns the independent SQLite migration history so repository ports can be
//! added without mixing SQL dialects or changing PostgreSQL checksums.

use std::{fmt, str::FromStr};

use rust_decimal::Decimal;
use sqlx::{
    Decode, Encode, Sqlite, Type,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    types::Json,
};
use uuid::Uuid;

/// SQLite migration history for the optional backend implementation.
pub static SQLITE_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/sqlite");

/// Lossless SQLite TEXT representation for PostgreSQL-compatible NUMERIC data.
///
/// SQLx does not implement `rust_decimal` for SQLite. Repository queries must
/// use this adapter rather than binding an `f64`, which would lose billing
/// precision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SqliteDecimal(Decimal);

impl SqliteDecimal {
    #[must_use]
    pub const fn into_inner(self) -> Decimal {
        self.0
    }
}

impl From<Decimal> for SqliteDecimal {
    fn from(value: Decimal) -> Self {
        Self(value)
    }
}

impl From<SqliteDecimal> for Decimal {
    fn from(value: SqliteDecimal) -> Self {
        value.into_inner()
    }
}

impl fmt::Display for SqliteDecimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.normalize().fmt(formatter)
    }
}

impl Type<Sqlite> for SqliteDecimal {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }

    fn compatible(type_info: &SqliteTypeInfo) -> bool {
        <String as Type<Sqlite>>::compatible(type_info)
    }
}

impl<'query> Encode<'query, Sqlite> for SqliteDecimal {
    fn encode_by_ref(
        &self,
        arguments: &mut Vec<SqliteArgumentValue<'query>>,
    ) -> Result<sqlx::encode::IsNull, BoxDynError> {
        <String as Encode<Sqlite>>::encode(self.to_string(), arguments)
    }
}

impl Decode<'_, Sqlite> for SqliteDecimal {
    fn decode(value: SqliteValueRef<'_>) -> Result<Self, BoxDynError> {
        let value = <String as Decode<Sqlite>>::decode(value)?;
        Decimal::from_str(&value).map(Self).map_err(Into::into)
    }
}

/// JSON TEXT adapter for PostgreSQL `uuid[]` columns.
pub type SqliteUuidList = Json<Vec<Uuid>>;

/// JSON TEXT adapter for PostgreSQL `text[]` columns.
pub type SqliteStringList = Json<Vec<String>>;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rust_decimal::Decimal;
    use sqlx::{
        Row, SqlitePool,
        sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
        types::Json,
    };
    use uuid::Uuid;

    use super::{SQLITE_MIGRATOR, SqliteDecimal, SqliteStringList, SqliteUuidList};

    const DOMAIN_TABLES: [&str; 27] = [
        "api_key_policies",
        "api_keys",
        "audit_logs",
        "channel_groups",
        "channels",
        "codex_oauth_credential_channels",
        "codex_oauth_credentials",
        "codex_oauth_flows",
        "codex_quota_reset_events",
        "codex_quota_window_periods",
        "config_templates",
        "connector_pools",
        "mcp_servers",
        "model_rules",
        "models",
        "proxies",
        "registration_invitation_codes",
        "request_log_ingest",
        "request_logs",
        "spend_leaderboard_entries",
        "spend_leaderboard_periods",
        "system_settings",
        "user_group_codex_quota_visibility",
        "user_groups",
        "user_invitations",
        "user_sessions",
        "users",
    ];

    async fn migrated_memory_pool() -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5))
            .synchronous(SqliteSynchronous::Full);
        let pool = SqlitePoolOptions::new()
            // Distinct SQLite in-memory connections do not share one schema.
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        SQLITE_MIGRATOR.run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn current_schema_migrates_with_foreign_keys_and_seed_groups() {
        assert_eq!(SQLITE_MIGRATOR.iter().count(), 1);
        assert_eq!(crate::persistence::POSTGRES_MIGRATOR.iter().count(), 49);

        let pool = migrated_memory_pool().await;

        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);

        let tables = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_schema \
             WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' \
             ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(tables, DOMAIN_TABLES);

        let groups = sqlx::query_as::<_, (String, String)>(
            "SELECT lower(hex(id)),system_role FROM user_groups ORDER BY system_role",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            groups,
            vec![
                ("00000000000000000000000000000102".into(), "admin".into()),
                ("00000000000000000000000000000101".into(), "user".into()),
            ]
        );

        sqlx::query(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids) VALUES (?1,'before','[]','[]')",
        )
        .bind(Uuid::from_u128(0x200))
        .execute(&pool)
        .await
        .unwrap();
        let before: String =
            sqlx::query_scalar("SELECT updated_at FROM api_key_policies WHERE id=?1")
                .bind(Uuid::from_u128(0x200))
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE api_key_policies SET name='after' WHERE id=?1")
            .bind(Uuid::from_u128(0x200))
            .execute(&pool)
            .await
            .unwrap();
        let after: String =
            sqlx::query_scalar("SELECT updated_at FROM api_key_policies WHERE id=?1")
                .bind(Uuid::from_u128(0x200))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(after > before, "{after} should be newer than {before}");
    }

    #[tokio::test]
    async fn schema_enforces_json_foreign_keys_and_append_only_audits() {
        let pool = migrated_memory_pool().await;

        let invalid_json = sqlx::query(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids) VALUES (?1,?2,?3,?4)",
        )
        .bind(Uuid::from_u128(0x201))
        .bind("invalid-json")
        .bind("not-json")
        .bind("[]")
        .execute(&pool)
        .await;
        assert!(invalid_json.is_err());

        let null_collection_item = sqlx::query(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids) VALUES (?1,?2,'[null]','[]')",
        )
        .bind(Uuid::from_u128(0x207))
        .bind("null-collection-item")
        .execute(&pool)
        .await;
        assert!(null_collection_item.is_err());

        let invalid_uuid = sqlx::query(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids) VALUES (?1,?2,'[]','[]')",
        )
        .bind(vec![0_u8; 15])
        .bind("invalid-uuid")
        .execute(&pool)
        .await;
        assert!(invalid_uuid.is_err());

        let missing_user = sqlx::query(
            "INSERT INTO api_keys \
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
             VALUES (?1,?2,?3,?4,'active',?5,?6)",
        )
        .bind(Uuid::from_u128(0x202))
        .bind(Uuid::from_u128(0x299))
        .bind("missing-user")
        .bind("secret")
        .bind("[\"open_ai_responses\"]")
        .bind("[\"proxy\"]")
        .execute(&pool)
        .await;
        assert!(missing_user.is_err());

        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_type,action,object_type,object_id,before_redacted,after_redacted) \
             VALUES (?1,'system','test','schema',?2,'{}','{}')",
        )
        .bind(Uuid::from_u128(0x203))
        .bind(Uuid::from_u128(0x204))
        .execute(&pool)
        .await
        .unwrap();

        let update = sqlx::query("UPDATE audit_logs SET action='changed' WHERE id=?1")
            .bind(Uuid::from_u128(0x203))
            .execute(&pool)
            .await;
        assert!(update.is_err());
        let delete = sqlx::query("DELETE FROM audit_logs WHERE id=?1")
            .bind(Uuid::from_u128(0x203))
            .execute(&pool)
            .await;
        assert!(delete.is_err());
    }

    #[tokio::test]
    async fn request_logs_allow_exactly_one_billing_transition() {
        let pool = migrated_memory_pool().await;
        let user_id = Uuid::from_u128(0x208);
        let api_key_id = Uuid::from_u128(0x209);
        let model_id = Uuid::from_u128(0x20a);
        let request_log_id = Uuid::from_u128(0x20b);

        sqlx::query("INSERT INTO users (id,display_name,balance_amount) VALUES (?1,?2,'10')")
            .bind(user_id)
            .bind("SQLite billing user")
            .execute(&pool)
            .await
            .unwrap();
        let negative_balance = SqliteDecimal::from(Decimal::from_str_exact("-0.25").unwrap());
        sqlx::query("UPDATE users SET balance_amount=?2 WHERE id=?1")
            .bind(user_id)
            .bind(negative_balance)
            .execute(&pool)
            .await
            .unwrap();
        let stored_balance =
            sqlx::query_scalar::<_, SqliteDecimal>("SELECT balance_amount FROM users WHERE id=?1")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored_balance, negative_balance);

        sqlx::query(
            "INSERT INTO api_keys \
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
             VALUES (?1,?2,?3,?4,'active',?5,?6)",
        )
        .bind(api_key_id)
        .bind(user_id)
        .bind("billing")
        .bind("sqlite-billing-secret")
        .bind("[\"open_ai_responses\"]")
        .bind("[\"proxy\"]")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO models \
             (id,source_model_id,display_name,price_unit_tokens,input_unit_price, \
              cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at) \
             VALUES (?1,?2,?3,1000000,'0','0','0','0',?4)",
        )
        .bind(model_id)
        .bind("sqlite-billing-model")
        .bind("SQLite billing model")
        .bind("2026-01-01T00:00:00.000Z")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation, \
              client_model,outcome,model_id,currency,price_unit_tokens,price_effective_at, \
              input_unit_price,cached_input_unit_price,cache_write_unit_price, \
              output_unit_price,cost_amount) \
             VALUES (?1,?2,?3,?4,?5,'open_ai_responses','responses',?6,'succeeded', \
                     ?7,'USD',1000000,?8,'0','0','0','0','0')",
        )
        .bind(request_log_id)
        .bind("2026-01-01T00:00:00.000Z")
        .bind("2026-01-01T00:00:01.000Z")
        .bind(user_id)
        .bind(api_key_id)
        .bind("sqlite-client-model")
        .bind(model_id)
        .bind("2026-01-01T00:00:00.000Z")
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("UPDATE request_logs SET billed_at=?2 WHERE id=?1")
            .bind(request_log_id)
            .bind("2026-01-01T00:00:02.000Z")
            .execute(&pool)
            .await
            .unwrap();
        let second_billing = sqlx::query("UPDATE request_logs SET billed_at=?2 WHERE id=?1")
            .bind(request_log_id)
            .bind("2026-01-01T00:00:03.000Z")
            .execute(&pool)
            .await;
        assert!(second_billing.is_err());
        let fact_update = sqlx::query("UPDATE request_logs SET outcome='failed' WHERE id=?1")
            .bind(request_log_id)
            .execute(&pool)
            .await;
        assert!(fact_update.is_err());
        let delete = sqlx::query("DELETE FROM request_logs WHERE id=?1")
            .bind(request_log_id)
            .execute(&pool)
            .await;
        assert!(delete.is_err());
    }

    #[tokio::test]
    async fn decimal_and_collection_columns_retain_lossless_text_storage() {
        let pool = migrated_memory_pool().await;
        let output_price = Decimal::from_str_exact("123456789012.123456789012").unwrap();

        sqlx::query(
            "INSERT INTO models \
             (id,source_model_id,display_name,price_unit_tokens,input_unit_price, \
              cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at) \
             VALUES (?1,?2,?3,1000000,?4,?5,?6,?7,?8)",
        )
        .bind(Uuid::from_u128(0x205))
        .bind("sqlite-test-model")
        .bind("SQLite test model")
        .bind("0.000000000001")
        .bind("0")
        .bind("0")
        .bind(SqliteDecimal::from(output_price))
        .bind("2026-01-01T00:00:00.000000Z")
        .execute(&pool)
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT typeof(id) AS id_type, \
                    typeof(output_unit_price) AS price_type,output_unit_price, \
                    typeof(source_payload) AS json_type \
             FROM models WHERE source_model_id='sqlite-test-model'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("id_type"), "blob");
        assert_eq!(row.get::<String, _>("price_type"), "text");
        assert_eq!(
            row.get::<String, _>("output_unit_price"),
            "123456789012.123456789012"
        );
        assert_eq!(row.get::<String, _>("json_type"), "text");

        let decoded = sqlx::query_scalar::<_, SqliteDecimal>(
            "SELECT output_unit_price FROM models WHERE source_model_id='sqlite-test-model'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(decoded.into_inner(), output_price);

        let excessive_integer_digits = sqlx::query(
            "UPDATE models SET output_unit_price=?1 \
             WHERE source_model_id='sqlite-test-model'",
        )
        .bind(SqliteDecimal::from(
            Decimal::from_str_exact("1234567890123").unwrap(),
        ))
        .execute(&pool)
        .await;
        assert!(excessive_integer_digits.is_err());
        let excessive_fractional_digits = sqlx::query(
            "UPDATE models SET output_unit_price=?1 \
             WHERE source_model_id='sqlite-test-model'",
        )
        .bind(SqliteDecimal::from(
            Decimal::from_str_exact("0.0000000000001").unwrap(),
        ))
        .execute(&pool)
        .await;
        assert!(excessive_fractional_digits.is_err());

        let group_ids: SqliteUuidList = Json(vec![Uuid::from_u128(0x101)]);
        let permissions: SqliteStringList = Json(vec!["proxy".into(), "models.read".into()]);
        sqlx::query(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids) VALUES (?1,?2,?3,'[]')",
        )
        .bind(Uuid::from_u128(0x206))
        .bind("sqlite-list-adapters")
        .bind(group_ids)
        .execute(&pool)
        .await
        .unwrap();
        let decoded_groups = sqlx::query_scalar::<_, SqliteUuidList>(
            "SELECT allowed_group_ids FROM api_key_policies WHERE id=?1",
        )
        .bind(Uuid::from_u128(0x206))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(decoded_groups.0, vec![Uuid::from_u128(0x101)]);

        let encoded_permissions = sqlx::query_scalar::<_, SqliteStringList>("SELECT ?1")
            .bind(permissions)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            encoded_permissions.0,
            vec!["proxy".to_owned(), "models.read".to_owned()]
        );
    }

    #[tokio::test]
    async fn file_connections_use_wal_and_full_synchronous_durability() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(directory.path().join("gateway.sqlite3"))
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

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 2);
    }
}
