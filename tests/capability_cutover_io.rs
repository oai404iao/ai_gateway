//! Isolated round trips for the database-backed capability cutover.
//!
//! Each test creates its own throwaway database, applies the not-yet-registered
//! canonical DDL, runs the real transfer, and reads the canonical rows back.
//! The shared `ai_gateway` database is never addressed.

#![cfg(target_os = "linux")]

use std::env;

use ai_gateway::persistence::capability_cutover::io::{CapabilityCutoverIoError, pg_transfer};
use ai_gateway::persistence::capability_cutover::transfer::{
    ChannelIdentityRegistryRecord, GroupIdentityRegistryRecord, RuleIdentityRegistryRecord,
};
use ai_gateway::persistence::{UpstreamTopologyRecords, pg_load, run_migrations};
use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Acquire, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const GROUP: Uuid = Uuid::from_u128(0x4000_0000_0000_0000_0000_0000_0000_0001);
const CHANNEL: Uuid = Uuid::from_u128(0x5000_0000_0000_0000_0000_0000_0000_0001);
const MODEL: Uuid = Uuid::from_u128(0x3000_0000_0000_0000_0000_0000_0000_0001);
const PROFILE: Uuid = Uuid::from_u128(0x6000_0000_0000_0000_0000_0000_0000_0001);
const RULE: Uuid = Uuid::from_u128(0x7000_0000_0000_0000_0000_0000_0000_0001);
const USER: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0001);
const KEY: Uuid = Uuid::from_u128(0x2000_0000_0000_0000_0000_0000_0000_0001);
const POLICY: Uuid = Uuid::from_u128(0x9000_0000_0000_0000_0000_0000_0000_0001);
const POOL: Uuid = Uuid::from_u128(0x8000_0000_0000_0000_0000_0000_0000_0001);
const CODEX_GROUP: Uuid = Uuid::from_u128(0x4000_0000_0000_0000_0000_0000_0000_0002);
const CODEX_CHANNEL: Uuid = Uuid::from_u128(0x5000_0000_0000_0000_0000_0000_0000_0002);
const CODEX_MODEL: Uuid = Uuid::from_u128(0x3000_0000_0000_0000_0000_0000_0000_0002);
const CODEX_PROFILE: Uuid = Uuid::from_u128(0x6000_0000_0000_0000_0000_0000_0000_0002);
const CODEX_RULE: Uuid = Uuid::from_u128(0x7000_0000_0000_0000_0000_0000_0000_0002);

fn cutover_at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).expect("valid cutover timestamp")
}

fn sorted<T: Serialize>(values: &[T]) -> Vec<Value> {
    let mut values = values
        .iter()
        .map(|value| serde_json::to_value(value).expect("canonical record is serializable"))
        .collect::<Vec<_>>();
    values.sort_by_key(|value| value.to_string());
    values
}

fn decode<T: for<'de> serde::Deserialize<'de>>(rows: Vec<String>) -> Vec<T> {
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).expect("persisted registry row decodes"))
        .collect()
}

fn assert_topology(expected: &UpstreamTopologyRecords, loaded: &UpstreamTopologyRecords) {
    assert_eq!(
        sorted(&expected.routing_groups),
        sorted(&loaded.routing_groups)
    );
    assert_eq!(
        sorted(&expected.upstream_accesses),
        sorted(&loaded.upstream_accesses)
    );
    assert_eq!(
        sorted(&expected.logical_channels),
        sorted(&loaded.logical_channels)
    );
    assert_eq!(
        sorted(&expected.channel_capabilities),
        sorted(&loaded.channel_capabilities)
    );
    assert_eq!(
        sorted(&expected.operation_rules),
        sorted(&loaded.operation_rules)
    );
    assert_eq!(
        sorted(&expected.operation_tiers),
        sorted(&loaded.operation_tiers)
    );
    assert_eq!(
        sorted(&expected.operation_candidates),
        sorted(&loaded.operation_candidates)
    );
    assert_eq!(
        sorted(&expected.api_key_grants),
        sorted(&loaded.api_key_grants)
    );
    assert_eq!(
        sorted(&expected.policy_grants),
        sorted(&loaded.policy_grants)
    );
}

fn access_input(
    record: &ai_gateway::persistence::UpstreamAccessRecord,
) -> ai_gateway::persistence::UpstreamAccessInput {
    ai_gateway::persistence::UpstreamAccessInput {
        name: record.name.clone(),
        connector_kind: record.connector_kind,
        base_url: record.base_url.clone(),
        proxy_id: record.proxy_id,
        connect_timeout_ms: record.connect_timeout_ms,
        response_header_timeout_ms: record.response_header_timeout_ms,
        stream_idle_timeout_ms: record.stream_idle_timeout_ms,
        enabled: record.enabled,
    }
}

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDatabase {
    async fn new() -> Self {
        let admin_url = env::var("TEST_DATABASE_ADMIN_URL")
            .expect("TEST_DATABASE_ADMIN_URL must be configured");
        let mut database_url = Url::parse(&admin_url).expect("valid PostgreSQL admin URL");
        assert_ne!(
            database_url.path().trim_matches('/'),
            "ai_gateway",
            "TEST_DATABASE_ADMIN_URL must not target the ai_gateway application database"
        );
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("configured PostgreSQL administrator database must be available");
        let name = format!("ai_gateway_cutover_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&admin)
            .await
            .expect("temporary database must be creatable");
        database_url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url.as_str())
            .await
            .expect("temporary database must be connectable");
        Self { pool, admin, name }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE \"{}\" WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("temporary database must be removable");
        self.admin.close().await;
    }
}

async fn seed(pool: &PgPool) {
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO users (id,display_name,email,role,status,password_hash)
         VALUES ($1,'Cutover Test','cutover@example.test','admin','active','test-hash')",
    )
    .bind(USER)
    .execute(&mut *transaction)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO models (id,source_model_id,display_name,currency,price_unit_tokens,
             input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
         VALUES ($1,'cutover-model','Cutover Model','USD',1000000,1,0,0,2,now()),
                ($2,'cutover-codex-model','Cutover Codex Model','USD',1000000,1,0,0,2,now())",
    )
    .bind(MODEL)
    .bind(CODEX_MODEL)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query("INSERT INTO model_routing_profiles (id,model_id) VALUES ($1,$2),($3,$4)")
        .bind(PROFILE)
        .bind(MODEL)
        .bind(CODEX_PROFILE)
        .bind(CODEX_MODEL)
        .execute(&mut *transaction)
        .await
        .unwrap();

    // Ordinary group and channel with a scheduled probe.
    sqlx::query(
        "INSERT INTO channel_groups (id,name,api_format,enabled)
         VALUES ($1,'cutover-ordinary','open_ai_chat_completions',true)",
    )
    .bind(GROUP)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channels (id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind,
             available_models,test_model,test_pricing_model_id)
         VALUES ($1,$2,'open_ai_chat_completions','cutover-chat','https://ordinary.test',true,'none',
             ARRAY['wire-model'],'wire-model',$3)",
    )
    .bind(CHANNEL)
    .bind(GROUP)
    .bind(MODEL)
    .execute(&mut *transaction)
    .await
    .unwrap();

    // Codex pool: the trigger creates the Images group, projections and channel.
    sqlx::query("INSERT INTO connector_pools (id,connector_kind) VALUES ($1,'codex_oauth')")
        .bind(POOL)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO channel_groups (id,name,api_format,connector_kind,connector_pool_id,enabled)
         VALUES ($1,'cutover-codex','open_ai_responses','codex_oauth',$2,true)",
    )
    .bind(CODEX_GROUP)
    .bind(POOL)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channels (id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind,
             available_models,supports_websocket,supports_standalone_web_search)
         VALUES ($1,$2,'open_ai_responses','cutover-codex','https://codex.test',true,'none',
             ARRAY['gpt-wire'],true,true)",
    )
    .bind(CODEX_CHANNEL)
    .bind(CODEX_GROUP)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO codex_oauth_credentials
             (channel_id,channel_group_id,label,account_id,id_token,access_token,refresh_token,last_refreshed_at)
         VALUES ($1,$2,'cutover codex','cutover-account','id-token','access-token','refresh-token',now())",
    )
    .bind(CODEX_CHANNEL)
    .bind(CODEX_GROUP)
    .execute(&mut *transaction)
    .await
    .unwrap();

    // Protocol rules and explicit candidate weights.
    sqlx::query(
        "INSERT INTO model_rules (id,api_format,model_routing_profile_id,enabled)
         VALUES ($1,'open_ai_chat_completions',$2,true),($3,'open_ai_responses',$4,true)",
    )
    .bind(RULE)
    .bind(PROFILE)
    .bind(CODEX_RULE)
    .bind(CODEX_PROFILE)
    .execute(&mut *transaction)
    .await
    .unwrap();
    for (rule_id, format, candidate_channel, upstream_model, weight) in [
        (RULE, "open_ai_chat_completions", CHANNEL, "wire-model", 5),
        (
            CODEX_RULE,
            "open_ai_responses",
            CODEX_CHANNEL,
            "gpt-wire",
            3,
        ),
    ] {
        sqlx::query(
            "INSERT INTO model_rule_routing_tiers (model_rule_id,api_format,priority,selection_strategy)
             VALUES ($1,$2::api_format,0,'weighted_round_robin')",
        )
        .bind(rule_id)
        .bind(format)
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO model_rule_routing_candidates
                 (model_rule_id,api_format,priority,channel_id,upstream_model,weight)
             VALUES ($1,$2::api_format,0,$3,$4,$5)",
        )
        .bind(rule_id)
        .bind(format)
        .bind(candidate_channel)
        .bind(upstream_model)
        .bind(weight)
        .execute(&mut *transaction)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO api_keys (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
             allowed_group_ids,allowed_channel_ids)
         VALUES ($1,$2,'cutover key','cutover-secret','active',ARRAY['open_ai_chat_completions']::api_format[],
             ARRAY['proxy'],ARRAY[$3]::uuid[],ARRAY[]::uuid[])",
    )
    .bind(KEY)
    .bind(USER)
    .bind(GROUP)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_key_policies (id,name,allowed_group_ids,allowed_channel_ids)
         VALUES ($1,'cutover policy',ARRAY[$2]::uuid[],ARRAY[]::uuid[])",
    )
    .bind(POLICY)
    .bind(GROUP)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
}

#[tokio::test]
async fn postgres_cutover_round_trips_legacy_configuration() {
    let database = TestDatabase::new().await;
    run_migrations(&database.pool)
        .await
        .expect("migrations must apply to the temporary database");
    seed(&database.pool).await;

    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::raw_sql(include_str!(
        "../src/persistence/capability_cutover/postgres-schema.sql"
    ))
    .execute(&mut *transaction)
    .await
    .expect("canonical DDL must apply to the temporary database");

    let output = pg_transfer(&mut transaction, cutover_at())
        .await
        .expect("capability cutover must transfer");
    let loaded = pg_load(&mut transaction)
        .await
        .expect("canonical topology must load");

    assert_eq!(output.topology.operation_rules.len(), 3);
    assert_eq!(output.topology.routing_groups.len(), 2);
    assert_eq!(output.topology.api_key_grants.len(), 1);
    assert_eq!(output.topology.policy_grants.len(), 1);
    assert_topology(&output.topology, &loaded);
    let rule = loaded
        .operation_rules
        .iter()
        .find(|rule| rule.id == RULE)
        .unwrap();
    let mut input = ai_gateway::persistence::upstream_topology::rules::rule_input(&loaded, rule);
    input.routing_tiers[0].candidates[0].weight = 11;
    ai_gateway::persistence::upstream_topology::rules::pg_save(
        &mut transaction,
        RULE,
        &input,
        Some(rule.updated_at),
    )
    .await
    .unwrap();
    assert!(matches!(
        ai_gateway::persistence::upstream_topology::rules::pg_save(
            &mut transaction,
            RULE,
            &input,
            Some(rule.updated_at),
        )
        .await,
        Err(ai_gateway::persistence::RepositoryError::Conflict)
    ));
    let replaced = pg_load(&mut transaction).await.unwrap();
    assert_eq!(
        sorted(&loaded.api_key_grants),
        sorted(&replaced.api_key_grants)
    );
    assert_eq!(
        sorted(&loaded.policy_grants),
        sorted(&replaced.policy_grants)
    );
    assert_eq!(
        sorted(&loaded.channel_capabilities),
        sorted(&replaced.channel_capabilities)
    );
    assert_eq!(
        replaced
            .operation_candidates
            .iter()
            .find(|candidate| candidate.upstream_model == "wire-model")
            .unwrap()
            .weight,
        11
    );
    let records =
        ai_gateway::persistence::upstream_topology::pg_load_control_plane(&mut transaction)
            .await
            .expect("canonical records must resolve credentials and grants");
    assert_compiled_snapshot(records);
    let logical = loaded
        .logical_channels
        .iter()
        .find(|channel| channel.id == CHANNEL)
        .unwrap();
    let access = loaded
        .upstream_accesses
        .iter()
        .find(|access| access.id == logical.access_id)
        .unwrap();
    let mut changed_access = access_input(access);
    changed_access.name = "Renamed access".into();
    let mutation = ai_gateway::persistence::upstream_topology::accesses::pg_save(
        &mut transaction,
        access.id,
        &changed_access,
        Some(access.updated_at),
    )
    .await
    .unwrap();
    assert!(
        !mutation
            .after_redacted
            .to_string()
            .contains(&access.base_url)
    );
    assert_ne!(
        mutation.after_redacted["revision"],
        access.revision.to_string()
    );
    assert!(matches!(
        ai_gateway::persistence::upstream_topology::accesses::pg_save(
            &mut transaction,
            access.id,
            &changed_access,
            Some(access.updated_at),
        )
        .await,
        Err(ai_gateway::persistence::RepositoryError::Conflict)
    ));
    let mut savepoint = transaction.begin().await.unwrap();
    let credential = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO upstream_credentials(id,name,kind,secret,allowed_base_urls)
         VALUES ($1,'Scope test','bearer','synthetic-test-secret',jsonb_build_array($2::text))",
    )
    .bind(credential)
    .bind(&access.base_url)
    .execute(&mut *savepoint)
    .await
    .unwrap();
    sqlx::query("UPDATE upstream_channels SET credential_id=$2,enabled=false,binding_revision=$3 WHERE id=$1")
        .bind(logical.id).bind(credential).bind(Uuid::new_v4()).execute(&mut *savepoint).await.unwrap();
    changed_access.base_url = "https://outside-scope.test".into();
    assert!(
        ai_gateway::persistence::upstream_topology::accesses::pg_save(
            &mut savepoint,
            access.id,
            &changed_access,
            Some(mutation.updated_at),
        )
        .await
        .is_err()
    );
    savepoint.rollback().await.unwrap();
    let group_history = decode::<GroupIdentityRegistryRecord>(
        sqlx::query_scalar::<_, String>(
            "SELECT jsonb_build_object('id',id,'label',label,'created_at',created_at,
                 'canonical_group_id',canonical_group_id)::text
             FROM group_identity_registry",
        )
        .fetch_all(&mut *transaction)
        .await
        .unwrap(),
    );
    let channel_history = decode::<ChannelIdentityRegistryRecord>(
        sqlx::query_scalar::<_, String>(
            "SELECT jsonb_build_object('id',id,'label',label,'created_at',created_at,
                 'canonical_channel_id',canonical_channel_id,'codex_credential_id',codex_credential_id,
                 'capability_id',capability_id)::text
             FROM channel_identity_registry",
        )
        .fetch_all(&mut *transaction)
        .await
        .unwrap(),
    );
    assert_eq!(
        sorted(&output.group_identity_registry),
        sorted(&group_history)
    );
    assert_eq!(
        sorted(&output.channel_identity_registry),
        sorted(&channel_history)
    );
    let rule_history = decode::<RuleIdentityRegistryRecord>(
        sqlx::query_scalar::<_, String>(
            "SELECT row_to_json(r)::text FROM model_rule_identity_registry r",
        )
        .fetch_all(&mut *transaction)
        .await
        .unwrap(),
    );
    assert_eq!(
        sorted(&output.rule_identity_registry),
        sorted(&rule_history)
    );
    assert_eq!(rule_history.len(), 3);

    sqlx::query(
        "INSERT INTO request_metering_facts
         (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
          request_protocol,client_model,outcome,peak_pricing,cost_amount,
          channel_group_id,channel_id,model_rule_id)
         VALUES ($1,now(),now(),$2,$3,'client','open_ai_chat_completions','chat_completions',
                 'non_stream','cutover-model','failed',false,0,$4,$5,$6)",
    )
    .bind(Uuid::from_u128(0xa000))
    .bind(USER)
    .bind(KEY)
    .bind(GROUP)
    .bind(CHANNEL)
    .bind(RULE)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::raw_sql(
        "INSERT INTO request_logs
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,
          outcome,cost_amount,channel_group_id,channel_id,model_rule_id)
         SELECT id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,
                outcome,cost_amount,channel_group_id,channel_id,model_rule_id FROM request_metering_facts;
         INSERT INTO request_settlement_pending(request_id,completed_at)
         SELECT id,completed_at FROM request_metering_facts;",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    let before: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(f)::text FROM request_metering_facts f
         UNION ALL SELECT row_to_json(l)::text FROM request_logs l
         UNION ALL SELECT row_to_json(p)::text FROM request_settlement_pending p ORDER BY 1",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    ai_gateway::persistence::capability_cutover::history::pg_retarget_history(&mut transaction)
        .await
        .unwrap();
    let after: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(f)::text FROM request_metering_facts f
         UNION ALL SELECT row_to_json(l)::text FROM request_logs l
         UNION ALL SELECT row_to_json(p)::text FROM request_settlement_pending p ORDER BY 1",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    assert_eq!(after, before);
    let targets: Vec<String> = sqlx::query_scalar(
        "SELECT confrelid::regclass::text FROM pg_constraint
         WHERE conrelid IN ('request_logs'::regclass,'request_metering_facts'::regclass)
           AND contype='f'",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    for (old, registry) in [
        ("channels", "channel_identity_registry"),
        ("channel_groups", "group_identity_registry"),
        ("model_rules", "model_rule_identity_registry"),
    ] {
        assert!(!targets.iter().any(|target| target == old));
        assert_eq!(
            targets.iter().filter(|target| *target == registry).count(),
            2
        );
    }
    sqlx::query("UPDATE model_rules SET enabled=false WHERE model_routing_profile_id=$1")
        .bind(CODEX_PROFILE)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query("UPDATE model_operation_rules SET enabled=false WHERE model_routing_profile_id=$1")
        .bind(CODEX_PROFILE)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query("UPDATE models SET enabled=false,deleted_at=now(),deleted_by=$2 WHERE id=$1")
        .bind(CODEX_MODEL)
        .bind(USER)
        .execute(&mut *transaction)
        .await
        .unwrap();
    let records =
        ai_gateway::persistence::upstream_topology::pg_load_control_plane(&mut transaction)
            .await
            .unwrap();
    let snapshot = ai_gateway::runtime_config::compile_control_plane(records).unwrap();
    assert!(
        snapshot
            .operation_rule(
                ai_gateway::domain::ApiOperation::Responses,
                "cutover-codex-model"
            )
            .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM model_rule_identity_registry")
            .fetch_one(&mut *transaction)
            .await
            .unwrap(),
        3
    );
    let credential_version: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM codex_oauth_credentials WHERE channel_id=$1")
            .bind(CODEX_CHANNEL)
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
    transaction.commit().await.unwrap();
    let repository = ai_gateway::persistence::ControlPlaneRepository::new(database.pool.clone());
    let change = repository
        .prepare_codex_credential_update(
            USER,
            CODEX_CHANNEL,
            ai_gateway::persistence::CodexCredentialUpdateInput {
                label: "Canonical audit label".into(),
                enabled: true,
                proxy_id: None,
                quota_threshold_percent: 90,
            },
            credential_version,
        )
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].after_redacted["base_url"], "[REDACTED]");
    assert_eq!(
        mutations[0].after_redacted["capabilities"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        mutations[0].before_redacted["access_id"],
        mutations[0].after_redacted["access_id"]
    );
    assert_eq!(
        mutations[0].before_redacted["access_revision"],
        mutations[0].after_redacted["access_revision"]
    );
    database.cleanup().await;
}

fn assert_compiled_snapshot(records: ai_gateway::persistence::ControlPlaneRecords) {
    use ai_gateway::domain::{ApiOperation, CapabilityTransport};
    assert!(
        records
            .api_keys
            .iter()
            .all(|key| key.allowed_group_ids.is_empty())
    );
    let config = ai_gateway::runtime_config::compile_control_plane(records)
        .expect("canonical snapshot must compile");
    assert!(
        config
            .operation_rule(ApiOperation::ChatCompletions, "cutover-model")
            .is_some()
    );
    assert!(
        config
            .operation_rule(ApiOperation::Responses, "cutover-codex-model")
            .is_some()
    );
    assert!(
        config
            .operation_rule(ApiOperation::StandaloneWebSearch, "cutover-codex-model")
            .is_some()
    );
    assert!(
        config
            .operation_rule(ApiOperation::ImagesGeneration, "cutover-codex-model")
            .is_none()
    );
    for channel in config
        .channels()
        .filter(|channel| channel.credential_id() == Some(CODEX_CHANNEL))
    {
        assert_eq!(channel.logical_channel_id(), CODEX_CHANNEL);
        if channel.api_operation() == ApiOperation::StandaloneWebSearch {
            assert!(channel.permits_transport(CapabilityTransport::HttpJson));
            assert!(!channel.permits_transport(CapabilityTransport::HttpSse));
        }
    }
}
#[tokio::test]
async fn postgres_cutover_refuses_a_nonempty_destination() {
    let database = TestDatabase::new().await;
    run_migrations(&database.pool)
        .await
        .expect("migrations must apply to the temporary database");
    seed(&database.pool).await;

    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::raw_sql(include_str!(
        "../src/persistence/capability_cutover/postgres-schema.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO routing_groups (id,name,enabled,sharing_only) VALUES ($1,'existing',true,false)",
    )
    .bind(Uuid::new_v4())
    .execute(&mut *transaction)
    .await
    .unwrap();

    assert!(matches!(
        pg_transfer(&mut transaction, cutover_at()).await,
        Err(CapabilityCutoverIoError::DestinationNotEmpty)
    ));

    transaction.rollback().await.unwrap();
    database.cleanup().await;
}

#[cfg(feature = "sqlite-backend")]
mod sqlite_backend {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt};

    use ai_gateway::persistence::capability_cutover::io::sqlite_transfer;
    use ai_gateway::persistence::capability_cutover::transfer::{
        ChannelIdentityRegistryRecord, GroupIdentityRegistryRecord,
    };
    use ai_gateway::persistence::sqlite::SqliteDatabase;
    use ai_gateway::persistence::sqlite_load;

    use super::*;

    const PRICE_EFFECTIVE_AT: &str = "2026-01-01T00:00:00.000000Z";

    async fn database() -> (tempfile::TempDir, SqliteDatabase) {
        let directory = tempfile::Builder::new()
            .permissions(Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let database = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .unwrap();
        (directory, database)
    }

    async fn seed(transaction: &mut sqlx::Transaction<'static, sqlx::Sqlite>) {
        let id = |value: Uuid| value.to_string();
        sqlx::query(
            "INSERT INTO users (id,display_name,email,role,status,password_hash)
             VALUES (?1,'Cutover Test','cutover@example.test','admin','active','test-hash')",
        )
        .bind(id(USER))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO models (id,source_model_id,display_name,price_unit_tokens,
                 input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
             VALUES (?1,'cutover-model','Cutover Model',1000000,'1','0','0','2',?2)",
        )
        .bind(id(MODEL))
        .bind(PRICE_EFFECTIVE_AT)
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query("INSERT INTO model_routing_profiles (id,model_id) VALUES (?1,?2)")
            .bind(id(PROFILE))
            .bind(id(MODEL))
            .execute(&mut **transaction)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_groups (id,name,api_format,enabled)
             VALUES (?1,'cutover-ordinary','open_ai_chat_completions',1)",
        )
        .bind(id(GROUP))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO channels (id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind,
                 available_models,test_model,test_pricing_model_id)
             VALUES (?1,?2,'open_ai_chat_completions','cutover-chat','https://ordinary.test',1,'none',
                 '[\"wire-model\"]','wire-model',?3)",
        )
        .bind(id(CHANNEL))
        .bind(id(GROUP))
        .bind(id(MODEL))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO model_rules (id,api_format,model_routing_profile_id,enabled)
             VALUES (?1,'open_ai_chat_completions',?2,1)",
        )
        .bind(id(RULE))
        .bind(id(PROFILE))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO model_rule_routing_tiers (model_rule_id,api_format,priority,selection_strategy)
             VALUES (?1,'open_ai_chat_completions',0,'weighted_round_robin')",
        )
        .bind(id(RULE))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO model_rule_routing_candidates
                 (model_rule_id,api_format,priority,channel_id,upstream_model,weight)
             VALUES (?1,'open_ai_chat_completions',0,?2,'wire-model',5)",
        )
        .bind(id(RULE))
        .bind(id(CHANNEL))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO api_keys (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
                 allowed_group_ids,allowed_channel_ids)
             VALUES (?1,?2,'cutover key','cutover-secret','active','[\"open_ai_chat_completions\"]',
                 '[\"proxy\"]',?3,'[]')",
        )
        .bind(id(KEY))
        .bind(id(USER))
        .bind(format!("[\"{}\"]", id(GROUP)))
        .execute(&mut **transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO api_key_policies (id,name,allowed_group_ids,allowed_channel_ids)
             VALUES (?1,'cutover policy',?2,'[]')",
        )
        .bind(id(POLICY))
        .bind(format!("[\"{}\"]", id(GROUP)))
        .execute(&mut **transaction)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sqlite_cutover_round_trips_legacy_configuration() {
        let (_directory, database) = database().await;
        database
            .install_schema()
            .await
            .expect("migrations must apply to the temporary database");

        let mut transaction = database.begin_write().await.unwrap();
        sqlx::raw_sql(include_str!(
            "../src/persistence/capability_cutover/sqlite-schema.sql"
        ))
        .execute(&mut *transaction)
        .await
        .expect("canonical DDL must apply to the temporary database");
        seed(&mut transaction).await;

        let output = sqlite_transfer(&mut transaction, cutover_at())
            .await
            .expect("capability cutover must transfer");
        let loaded = sqlite_load(&mut transaction)
            .await
            .expect("canonical topology must load");

        assert_eq!(output.topology.operation_rules.len(), 1);
        assert_eq!(output.topology.routing_groups.len(), 1);
        assert_eq!(output.topology.api_key_grants.len(), 1);
        assert_eq!(output.topology.policy_grants.len(), 1);
        assert_topology(&output.topology, &loaded);
        let rule = loaded
            .operation_rules
            .iter()
            .find(|rule| rule.id == RULE)
            .unwrap();
        let mut input =
            ai_gateway::persistence::upstream_topology::rules::rule_input(&loaded, rule);
        input.routing_tiers[0].candidates[0].weight = 11;
        ai_gateway::persistence::upstream_topology::rules::sqlite_save(
            &mut transaction,
            RULE,
            &input,
            Some(rule.updated_at),
        )
        .await
        .unwrap();
        assert!(matches!(
            ai_gateway::persistence::upstream_topology::rules::sqlite_save(
                &mut transaction,
                RULE,
                &input,
                Some(rule.updated_at),
            )
            .await,
            Err(ai_gateway::persistence::RepositoryError::Conflict)
        ));
        let replaced = sqlite_load(&mut transaction).await.unwrap();
        assert_eq!(
            sorted(&loaded.api_key_grants),
            sorted(&replaced.api_key_grants)
        );
        assert_eq!(
            sorted(&loaded.policy_grants),
            sorted(&replaced.policy_grants)
        );
        assert_eq!(
            sorted(&loaded.channel_capabilities),
            sorted(&replaced.channel_capabilities)
        );
        assert_eq!(replaced.operation_candidates[0].weight, 11);
        let records =
            ai_gateway::persistence::upstream_topology::sqlite_load_control_plane(&mut transaction)
                .await
                .expect("canonical records must resolve credentials and grants");
        assert!(
            records
                .api_keys
                .iter()
                .all(|key| key.allowed_group_ids.is_empty())
        );
        let snapshot = ai_gateway::runtime_config::compile_control_plane(records)
            .expect("canonical SQLite snapshot must compile");
        assert!(
            snapshot
                .operation_rule(
                    ai_gateway::domain::ApiOperation::ChatCompletions,
                    "cutover-model",
                )
                .is_some()
        );
        let logical = &loaded.logical_channels[0];
        let access = &loaded.upstream_accesses[0];
        let mut changed_access = access_input(access);
        changed_access.name = "Renamed access".into();
        sqlx::query("SELECT ag_set_time(?)")
            .bind(access.updated_at.timestamp_micros() + 1_000_000)
            .execute(&mut *transaction)
            .await
            .unwrap();
        let mutation = ai_gateway::persistence::upstream_topology::accesses::sqlite_save(
            &mut transaction,
            access.id,
            &changed_access,
            Some(access.updated_at),
        )
        .await
        .unwrap();
        assert!(
            !mutation
                .after_redacted
                .to_string()
                .contains(&access.base_url)
        );
        assert_ne!(
            mutation.after_redacted["revision"],
            access.revision.to_string()
        );
        assert!(matches!(
            ai_gateway::persistence::upstream_topology::accesses::sqlite_save(
                &mut transaction,
                access.id,
                &changed_access,
                Some(access.updated_at),
            )
            .await,
            Err(ai_gateway::persistence::RepositoryError::Conflict)
        ));
        let mut savepoint = transaction.begin().await.unwrap();
        let credential = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO upstream_credentials(id,name,kind,secret,allowed_base_urls)
             VALUES (?,'Scope test','bearer','synthetic-test-secret',json_array(?))",
        )
        .bind(credential.to_string())
        .bind(&access.base_url)
        .execute(&mut *savepoint)
        .await
        .unwrap();
        sqlx::query("UPDATE upstream_channels SET credential_id=?,enabled=0,binding_revision=?,updated_at=ag_now() WHERE id=?")
            .bind(credential.to_string()).bind(Uuid::new_v4().to_string()).bind(logical.id.to_string())
            .execute(&mut *savepoint).await.unwrap();
        changed_access.base_url = "https://outside-scope.test".into();
        assert!(
            ai_gateway::persistence::upstream_topology::accesses::sqlite_save(
                &mut savepoint,
                access.id,
                &changed_access,
                Some(mutation.updated_at),
            )
            .await
            .is_err()
        );
        savepoint.rollback().await.unwrap();

        let group_history = decode::<GroupIdentityRegistryRecord>(
            sqlx::query_scalar::<_, String>(
                "SELECT json_object('id',id,'label',label,'created_at',created_at,
                     'canonical_group_id',canonical_group_id)
                 FROM group_identity_registry",
            )
            .fetch_all(&mut *transaction)
            .await
            .unwrap(),
        );
        let channel_history = decode::<ChannelIdentityRegistryRecord>(
            sqlx::query_scalar::<_, String>(
                "SELECT json_object('id',id,'label',label,'created_at',created_at,
                     'canonical_channel_id',canonical_channel_id,'codex_credential_id',codex_credential_id,
                     'capability_id',capability_id)
                 FROM channel_identity_registry",
            )
            .fetch_all(&mut *transaction)
            .await
            .unwrap(),
        );
        assert_eq!(
            sorted(&output.group_identity_registry),
            sorted(&group_history)
        );
        assert_eq!(
            sorted(&output.channel_identity_registry),
            sorted(&channel_history)
        );
        let rule_history = decode::<RuleIdentityRegistryRecord>(
            sqlx::query_scalar::<_, String>(
                "SELECT json_object('id',id,'label',label,'created_at',created_at,
                    'canonical_rule_id',canonical_rule_id) FROM model_rule_identity_registry",
            )
            .fetch_all(&mut *transaction)
            .await
            .unwrap(),
        );
        assert_eq!(
            sorted(&output.rule_identity_registry),
            sorted(&rule_history)
        );
        assert_eq!(rule_history.len(), 1);

        sqlx::raw_sql(
            "UPDATE channels SET test_model=NULL,test_pricing_model_id=NULL,updated_at=ag_now();
             UPDATE channel_capabilities SET test_model=NULL,test_pricing_model_id=NULL,updated_at=ag_now();
             UPDATE model_rules SET enabled=0,updated_at=ag_now();
             UPDATE model_operation_rules SET enabled=0,updated_at=ag_now();",
        ).execute(&mut *transaction).await.unwrap();
        sqlx::query("UPDATE models SET enabled=0,deleted_at=ag_now(),deleted_by=?,updated_at=ag_now() WHERE id=?")
            .bind(USER.to_string()).bind(MODEL.to_string()).execute(&mut *transaction).await.unwrap();
        let records =
            ai_gateway::persistence::upstream_topology::sqlite_load_control_plane(&mut transaction)
                .await
                .unwrap();
        let snapshot = ai_gateway::runtime_config::compile_control_plane(records).unwrap();
        assert!(
            snapshot
                .operation_rule(
                    ai_gateway::domain::ApiOperation::ChatCompletions,
                    "cutover-model"
                )
                .is_none()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM model_rule_identity_registry")
                .fetch_one(&mut *transaction)
                .await
                .unwrap(),
            1
        );
        transaction.rollback().await.unwrap();
        database.close().await;
    }
}
