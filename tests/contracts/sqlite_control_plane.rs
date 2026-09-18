//! SQLite ordinary control-plane repository contracts.
//!
//! These run against a real installed business schema and exercise the neutral
//! repository surface the Console API and workers depend on: coherent runtime
//! snapshots, list/detail/audit reads, deletion-impact tokens, self-service
//! options, prepared mutations with ETag conflicts, and rollback behavior when
//! snapshot compilation or audit writing fails before commit.
//!
//! Host wiring (the parent-owned `tests/sqlite_foundation.rs`):
//!
//! ```ignore
//! use chrono::{DateTime, Utc};
//! use ai_gateway::persistence::{DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID};
//! #[path = "contracts/sqlite_control_plane.rs"]
//! mod sqlite_control_plane;
//! ```
//!
//! The module reuses the host's `database()` helper and the `sqlite-backend`
//! feature gate declared there.

use super::*;
use ai_gateway::{
    persistence::{
        ChannelGroupInput, ControlPlaneMutation, ProxyCreateInput, RepositoryError,
        SelfApiKeyCreate, SelfApiKeyUpdate, SystemSettingsInput, UserSettingsInput,
        sqlite::{SqliteControlPlaneRepository, SqliteUuid},
    },
    runtime_config::compile_runtime_config,
};
use rust_decimal::Decimal;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const ADMIN: Uuid = Uuid::from_u128(0x401);
const USER: Uuid = Uuid::from_u128(0x402);
const KEY: Uuid = Uuid::from_u128(0x411);
const MODEL: Uuid = Uuid::from_u128(0x421);
const PROFILE: Uuid = Uuid::from_u128(0x422);
const RULE: Uuid = Uuid::from_u128(0x423);
const GROUP: Uuid = Uuid::from_u128(0x431);
const CHANNEL: Uuid = Uuid::from_u128(0x432);
const TEMPLATE: Uuid = Uuid::from_u128(0x441);
const PROXY: Uuid = Uuid::from_u128(0x451);
const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA";

async fn repository() -> (
    tempfile::TempDir,
    Arc<SqliteDatabase>,
    SqliteControlPlaneRepository,
) {
    let (directory, database) = database().await;
    assert_eq!(database.install_schema().await.unwrap(), 2);
    let database = Arc::new(database);
    let repository = SqliteControlPlaneRepository::new(Arc::clone(&database));
    (directory, database, repository)
}

async fn execute(database: &SqliteDatabase, sql: &str) {
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::Executor::execute(&mut *transaction, sql)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
}

async fn execute_expect_error(database: &SqliteDatabase, sql: &str) -> Option<String> {
    let mut transaction = database.begin_write().await.unwrap();
    let result = sqlx::Executor::execute(&mut *transaction, sql).await;
    match result {
        Ok(_) => {
            transaction.commit().await.unwrap();
            None
        }
        Err(error) => {
            let message = error.to_string();
            transaction.rollback().await.unwrap();
            Some(message)
        }
    }
}

/// Control-plane audits, excluding the one-time `system_settings` initialize
/// row written by `ensure_system_settings`.
async fn control_plane_audits(
    repository: &SqliteControlPlaneRepository,
) -> Vec<ai_gateway::persistence::ConsoleAuditLog> {
    repository
        .audit_logs(100)
        .await
        .unwrap()
        .into_iter()
        .filter(|audit| audit.object_type != "system_settings")
        .collect()
}

async fn scalar<T>(database: &SqliteDatabase, sql: &str) -> T
where
    for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
{
    let mut reader = database.acquire_read().await.unwrap();
    sqlx::query_scalar(sql)
        .fetch_one(&mut *reader)
        .await
        .unwrap()
}

async fn seed_admin_and_user(database: &SqliteDatabase) {
    execute(
        database,
        &format!(
            "INSERT INTO users (id,email,display_name,role,status,password_hash,password_changed_at,user_group_id)
             VALUES ('{ADMIN}','admin@example.test','Control admin','admin','active','{PASSWORD_HASH}',ag_now(),'{}');
             INSERT INTO users (id,email,display_name,role,status,password_hash,password_changed_at,user_group_id)
             VALUES ('{USER}','user@example.test','Control user','user','active','{PASSWORD_HASH}',ag_now(),'{}');",
            DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID
        ),
    )
    .await;
}

/// Seeds a priced model with one enabled protocol rule and a ready candidate,
/// plus an ordinary channel group and channel the candidate points at.
async fn seed_routing(database: &SqliteDatabase) {
    execute(
        database,
        &format!(
            "INSERT INTO models
             (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,
              input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,
              price_effective_at,advanced_billing,source_payload)
             VALUES ('{MODEL}','gpt-test','Test Model','Test Provider',1,'USD',1000000,
              '1','0.5','0','2','2026-01-01T00:00:00.000000Z',
              '{{\"long_context_tiers\": [], \"request_multipliers\": []}}','{{}}');
             INSERT INTO model_routing_profiles (id,model_id) VALUES ('{PROFILE}','{MODEL}');
             INSERT INTO channel_groups (id,name,api_format,connector_kind,enabled)
             VALUES ('{GROUP}','Ordinary Group','open_ai_chat_completions','openai_compatible',1);
             INSERT INTO channels
             (id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind,available_models)
             VALUES ('{CHANNEL}','{GROUP}','open_ai_chat_completions','Ordinary Channel',
                     'https://upstream.example.test',1,'none','[\"gpt-test\"]');
             INSERT INTO model_rules (id,model_routing_profile_id,api_format,enabled)
             VALUES ('{RULE}','{PROFILE}','open_ai_chat_completions',1);
             INSERT INTO model_rule_routing_tiers (model_rule_id,api_format,priority,selection_strategy)
             VALUES ('{RULE}','open_ai_chat_completions',0,'weighted_random');
             INSERT INTO model_rule_routing_candidates
             (model_rule_id,api_format,priority,channel_id,upstream_model,weight)
             VALUES ('{RULE}','open_ai_chat_completions',0,'{CHANNEL}','gpt-test',1);"
        ),
    )
    .await;
}

async fn seed_template(database: &SqliteDatabase) {
    insert_template(database, false).await;
}

async fn insert_template(database: &SqliteDatabase, with_proxy: bool) {
    let document = serde_json::json!({
        "version": 1,
        "api_format": "open_ai_chat_completions",
        "request_headers": {"set": {"x-template": "on"}}
    });
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query(
        "INSERT INTO config_templates (id,name,description,document,enabled) VALUES (?,?,?,?,1)",
    )
    .bind(SqliteUuid(TEMPLATE))
    .bind("Template")
    .bind("Template description")
    .bind(document.to_string())
    .execute(&mut *transaction)
    .await
    .unwrap();
    if with_proxy {
        sqlx::query(
            "INSERT INTO proxies (id,name,proxy_url,username,password,no_proxy_hosts,enabled) \
             VALUES (?,?,?,?,?,?,1)",
        )
        .bind(SqliteUuid(PROXY))
        .bind("Proxy")
        .bind("http://user:secret@proxy.example.test:8080")
        .bind("user")
        .bind("secret")
        .bind(serde_json::json!(["localhost"]).to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();
}

async fn seed_template_and_proxy(database: &SqliteDatabase) {
    insert_template(database, true).await;
}

fn settings() -> SystemSettingsInput {
    serde_json::from_value(json!({
        "api_hosts": ["https://gateway.example.test"],
        "upstream": {
            "connect_timeout_seconds": 10,
            "response_header_timeout_seconds": 30,
            "stream_idle_timeout_seconds": 60
        },
        "passive_health": {"connection_failure_threshold": 3, "cooldown_seconds": 60},
        "session_affinity": {"enabled": false, "max_entries": 100000, "default_ttl_seconds": 3600, "rules": []},
        "codex": {
            "originator": "codex_cli_rs",
            "client_version": "0.1.0",
            "user_agent": "codex_cli_rs/0.1.0"
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn runtime_snapshot_is_coherent_and_includes_codex_sharing_state() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    seed_template(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();

    let records = repository.load_runtime().await.unwrap();
    assert_eq!(
        records.system_settings.setting_key,
        ai_gateway::persistence::FORWARDING_SETTINGS_KEY
    );
    assert_eq!(
        records.system_settings.value["api_hosts"][0],
        "https://gateway.example.test"
    );
    let control_plane = &records.control_plane;
    assert_eq!(control_plane.models.len(), 1);
    assert_eq!(control_plane.models[0].source_model_id, "gpt-test");
    assert_eq!(control_plane.models[0].input_unit_price, Decimal::ONE);
    assert_eq!(control_plane.groups.len(), 1);
    assert_eq!(control_plane.channels.len(), 1);
    assert_eq!(
        control_plane.channels[0].available_models,
        vec!["gpt-test".to_owned()]
    );
    let rule = &control_plane.model_rules[0];
    assert_eq!(rule.client_model, "gpt-test");
    assert_eq!(rule.routing_tiers.len(), 1);
    assert_eq!(rule.routing_tiers[0].candidates[0].channel_id, CHANNEL);
    assert!(control_plane.api_keys.is_empty());
    assert_eq!(
        control_plane.templates[0].document["api_format"],
        "open_ai_chat_completions"
    );
    assert!(records.sharing.is_empty());
    assert!(records.sharing_only_channels.is_empty());

    let compiled = compile_runtime_config(records).unwrap();
    assert_eq!(compiled.channels().count(), 1);
    assert!(
        compiled
            .model_rule(
                ai_gateway::domain::ApiFormat::OpenAiChatCompletions,
                "gpt-test"
            )
            .is_some()
    );

    // `load` reads the same control-plane records without system settings.
    let plain = repository.load().await.unwrap();
    assert_eq!(plain.models.len(), 1);
    assert_eq!(plain.model_rules[0].id, RULE);
}

#[tokio::test]
async fn console_reads_expose_lists_details_audit_and_self_service_options() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    seed_template_and_proxy(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();
    execute(
        &database,
        &format!(
            "INSERT INTO api_keys
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
              allowed_group_ids,allowed_channel_ids)
             VALUES ('{KEY}','{USER}','User key','sk-test-secret','active',
                     '[\"open_ai_chat_completions\"]','[\"proxy\"]','[]','[]');
             INSERT INTO audit_logs
             (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,
              before_redacted,after_redacted,correlation_id,reason)
             VALUES ('40100000-0000-0000-0000-000000000461','{ADMIN}','user','admin','create',
                     'api_key','{KEY}','{{}}','{{\"id\":\"{KEY}\"}}','corr-1','audit reason');"
        ),
    )
    .await;

    let lists = repository.control_plane_lists().await.unwrap();
    assert_eq!(lists.users.len(), 2);
    assert_eq!(lists.users[0].id, ADMIN);
    assert_eq!(lists.users[1].balance_amount.to_string(), "0");
    assert_eq!(lists.api_keys.len(), 1);
    assert_eq!(lists.api_keys[0].secret, "sk-test-secret");
    assert_eq!(lists.channels.len(), 1);
    assert!(!lists.channels[0].provider_managed);
    assert_eq!(lists.model_rules.len(), 1);
    assert_eq!(
        lists.model_rules[0].protocol_rules[0].routing_status,
        ai_gateway::persistence::ModelRuleRoutingStatus::Ready
    );
    assert_eq!(
        lists.model_rules[0].protocol_rules[0].active_candidate_count,
        1
    );
    assert_eq!(
        lists.model_rules[0].protocol_rules[0].target_candidate_count,
        1
    );
    assert_eq!(
        lists.model_rules[0].protocol_rules[0].routing_tiers.len(),
        1
    );
    assert_eq!(lists.proxies.len(), 1);
    // Proxy listing and audit strip the credential component.
    assert_eq!(lists.proxies[0].proxy_url, "http://proxy.example.test:8080");
    assert!(lists.proxies[0].credential_configured);
    assert_eq!(
        lists.config_templates[0].api_format.as_deref(),
        Some("open_ai_chat_completions")
    );
    // The system group carries its live member count.
    let group = lists
        .user_groups
        .iter()
        .find(|group| group.id == DEFAULT_USER_GROUP_ID)
        .unwrap();
    assert_eq!(group.member_count, 1);

    let detail = repository
        .control_plane_channel_detail(CHANNEL)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.name, "Ordinary Channel");
    assert_eq!(detail.api_format, "open_ai_chat_completions");
    assert!(detail.upstream_api_key.is_none());
    assert!(
        repository
            .control_plane_channel_detail(Uuid::nil())
            .await
            .unwrap()
            .is_none()
    );
    let template = repository
        .control_plane_config_template_detail(TEMPLATE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(template.document["api_format"], "open_ai_chat_completions");
    let proxy_record = repository.load().await.unwrap().proxies.remove(0);
    assert_eq!(proxy_record.username.as_deref(), Some("user"));
    assert!(proxy_record.password.is_some());

    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].action, "create");
    assert_eq!(audits[0].actor_role.as_deref(), Some("admin"));
    assert_eq!(audits[0].correlation_id.as_deref(), Some("corr-1"));
    assert_eq!(
        audits[0].after_redacted.as_ref().unwrap()["id"],
        KEY.to_string()
    );

    let keys = repository.own_api_keys(USER).await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].secret, "sk-test-secret");
    assert!(repository.own_api_keys(ADMIN).await.unwrap().is_empty());
    assert!(
        repository.own_api_key(ADMIN, KEY).await.unwrap().is_none(),
        "another user's key is not visible"
    );
    assert!(repository.own_api_key(USER, KEY).await.unwrap().is_some());

    // With no default policy the user has no selectable targets at all.
    assert!(matches!(
        repository.own_api_key_options(USER).await.err(),
        Some(RepositoryError::DefaultApiKeyPolicyRequired)
    ));

    assert_eq!(
        repository.model_source_ids().await.unwrap(),
        vec!["gpt-test"]
    );
    let us = repository.user_settings(USER).await.unwrap().unwrap();
    assert!(!us.websocket_enabled);
    assert!(
        repository
            .user_settings(Uuid::nil())
            .await
            .unwrap()
            .is_none()
    );
    let view = repository.system_settings().await.unwrap();
    assert_eq!(view.settings.api_hosts.len(), 1);
}

#[tokio::test]
async fn deletion_impact_tokens_are_stable_and_reject_tampering() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();
    execute(
        &database,
        &format!(
            "INSERT INTO api_keys
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
              allowed_group_ids,allowed_channel_ids)
             VALUES ('{KEY}','{USER}','Bound key','sk-bound','active',
                     '[\"open_ai_chat_completions\"]','[\"proxy\"]','[\"{GROUP}\"]','[]');"
        ),
    )
    .await;

    let impact = repository
        .channel_group_deletion_impact(GROUP)
        .await
        .unwrap();
    assert_eq!(impact.resource_type, "channel_group");
    assert_eq!(impact.channels.len(), 1);
    assert_eq!(impact.channels[0].id, CHANNEL);
    assert_eq!(impact.api_keys.len(), 1);
    assert_eq!(impact.model_protocol_rules.len(), 1);
    assert!(impact.model_protocol_rules[0].will_disable);
    assert_eq!(
        impact.model_protocol_rules[0].removed_channel_group_ids,
        vec![GROUP]
    );
    let repeated = repository
        .channel_group_deletion_impact(GROUP)
        .await
        .unwrap();
    assert_eq!(impact.confirmation_token, repeated.confirmation_token);
    assert!(impact.confirmation_token.starts_with("v1."));

    assert!(matches!(
        repository
            .channel_group_deletion_impact(Uuid::nil())
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));

    // A channel-level plan narrows the same impact to one channel.
    let channel_impact = repository.channel_deletion_impact(CHANNEL).await.unwrap();
    assert_eq!(channel_impact.resource_type, "channel");
    assert_eq!(channel_impact.channels.len(), 1);
    assert_eq!(channel_impact.channels[0].id, CHANNEL);
    assert!(
        channel_impact.api_keys.is_empty(),
        "the key is bound to the group, not the individual channel"
    );
    assert_eq!(channel_impact.model_protocol_rules.len(), 1);
    assert_eq!(
        channel_impact.model_protocol_rules[0].removed_channel_ids,
        vec![CHANNEL]
    );
    assert!(
        channel_impact.model_protocol_rules[0]
            .removed_channel_group_ids
            .is_empty(),
        "a single-channel deletion does not remove the group itself"
    );

    // The confirmation token is verified while the change is prepared; a
    // mismatched token never yields a prepared change.
    let tampered = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::DeleteGroup {
                id: GROUP,
                deleted_by: ADMIN,
                expected_updated_at: group_updated_at(&database).await,
                confirmation_token: "v1.bogus".into(),
            },
        )
        .await;
    assert!(matches!(
        tampered.err(),
        Some(RepositoryError::DeletionImpactChanged)
    ));
    assert_eq!(
        scalar::<i64>(
            &database,
            "SELECT count(*) FROM channel_groups WHERE id IS NOT NULL"
        )
        .await,
        1
    );

    let mut change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::DeleteGroup {
                id: GROUP,
                deleted_by: ADMIN,
                expected_updated_at: group_updated_at(&database).await,
                confirmation_token: impact.confirmation_token.clone(),
            },
        )
        .await
        .unwrap();
    let records = change.runtime_records().await.unwrap();
    assert!(
        records.control_plane.groups.is_empty(),
        "the prepared snapshot reflects the pending tombstone"
    );
    compile_runtime_config(records).unwrap();
    let (mutations, correlation_id) = change.commit().await.unwrap();
    assert_eq!(mutations.len(), 1);
    assert!(mutations[0].correlation_id.is_some());
    assert_ne!(correlation_id, Uuid::nil());
    assert!(
        mutations[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("channels deleted")
    );

    let after = repository.load().await.unwrap();
    assert!(after.groups.is_empty());
    assert!(after.channels.is_empty());
    assert!(after.model_rules[0].routing_tiers.is_empty());
    assert!(!after.model_rules[0].enabled);
    let key = repository.own_api_key(USER, KEY).await.unwrap().unwrap();
    assert!(
        key.allowed_group_ids.is_empty(),
        "bound self-service keys are unbound at deletion"
    );
    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits[0].object_type, "channel_group");
    assert_eq!(audits[0].action, "delete");
    assert_eq!(
        audits[0].correlation_id.as_deref(),
        Some(correlation_id.to_string().as_str())
    );
}

async fn rule_updated_at(database: &SqliteDatabase) -> DateTime<Utc> {
    let mut reader = database.acquire_read().await.unwrap();
    let text: String = sqlx::query_scalar(
        "SELECT updated_at FROM model_rules WHERE api_format='open_ai_chat_completions'",
    )
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    DateTime::parse_from_rfc3339(&text)
        .unwrap()
        .with_timezone(&Utc)
}

async fn group_updated_at(database: &SqliteDatabase) -> DateTime<Utc> {
    let mut reader = database.acquire_read().await.unwrap();
    let text: String = sqlx::query_scalar("SELECT updated_at FROM channel_groups WHERE id=?")
        .bind(SqliteUuid(GROUP))
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    DateTime::parse_from_rfc3339(&text)
        .unwrap()
        .with_timezone(&Utc)
}

#[tokio::test]
async fn prepared_mutations_validate_commit_and_roll_back_with_audit() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();

    // Invalid actor fails before any transaction work.
    assert!(matches!(
        repository
            .prepare_mutation(
                Uuid::nil(),
                ControlPlaneMutation::CreateProxy(proxy("blocked"))
            )
            .await
            .err(),
        Some(RepositoryError::InvalidActor)
    ));

    let mut change = repository
        .prepare_mutation(ADMIN, ControlPlaneMutation::CreateProxy(proxy("created")))
        .await
        .unwrap();
    let records = change.runtime_records().await.unwrap();
    assert_eq!(records.control_plane.proxies.len(), 1);
    let (mutations, correlation_id) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "create");
    assert_eq!(mutations[0].object_type, "proxy");
    assert!(mutations[0].created_secret.is_none());
    let created = repository.load().await.unwrap();
    assert_eq!(created.proxies[0].name, "created");
    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].actor_user_id, Some(ADMIN));
    assert_eq!(
        audits[0].correlation_id.as_deref(),
        Some(correlation_id.to_string().as_str())
    );
    assert_eq!(audits[0].before_redacted, Some(json!({})));

    // Dropping a prepared change rolls it back without writing audit rows.
    let mut change = repository
        .prepare_mutation(ADMIN, ControlPlaneMutation::CreateProxy(proxy("dropped")))
        .await
        .unwrap();
    assert_eq!(
        change
            .runtime_records()
            .await
            .unwrap()
            .control_plane
            .proxies
            .len(),
        2
    );
    drop(change);
    let after_drop = repository.load().await.unwrap();
    assert_eq!(after_drop.proxies.len(), 1);
    assert_eq!(control_plane_audits(&repository).await.len(), 1);

    // An explicit rollback behaves like drop and keeps the schema unchanged.
    let change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::CreateProxy(proxy("rolled-back")),
        )
        .await
        .unwrap();
    change.rollback().await.unwrap();
    assert_eq!(repository.load().await.unwrap().proxies.len(), 1);

    // A manual reload prepares no mutation but records one audit entry.
    let change = repository.prepare_manual_reload(ADMIN).await.unwrap();
    let (mutations, correlation_id) = change.commit().await.unwrap();
    assert!(mutations.is_empty());
    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits[0].action, "reload");
    assert_eq!(audits[0].object_type, "runtime_config");
    assert_eq!(audits[0].object_id, Uuid::nil());
    assert_eq!(
        audits[0].correlation_id.as_deref(),
        Some(correlation_id.to_string().as_str())
    );

    repository.verify_active_admin(ADMIN).await.unwrap();
    assert!(matches!(
        repository.verify_active_admin(USER).await.err(),
        Some(RepositoryError::InvalidActor)
    ));
}

fn proxy(name: &str) -> ProxyCreateInput {
    ProxyCreateInput {
        name: name.into(),
        proxy_url: "http://proxy.example.test:8080".into(),
        username: None,
        password: None,
        no_proxy_hosts: Vec::new(),
        enabled: true,
    }
}

#[tokio::test]
async fn snapshot_compilation_failure_rolls_back_the_pending_change() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();

    // Removing the only channel of an enabled rule leaves the pending snapshot
    // uncompilable, so the caller must roll back instead of committing.
    let mut change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::UpdateGroup {
                id: GROUP,
                input: ChannelGroupInput {
                    name: "Ordinary Group".into(),
                    api_format: "open_ai_chat_completions".into(),
                    connector_kind: "openai_compatible".into(),
                    request_compression: None,
                    sharing_only: None,
                    enabled: false,
                    status_statistics_enabled: None,
                },
                expected_updated_at: group_updated_at(&database).await,
            },
        )
        .await
        .unwrap();
    let records = change.runtime_records().await.unwrap();
    assert!(!records.control_plane.groups[0].enabled);
    assert!(
        compile_runtime_config(records).is_ok(),
        "the malformed snapshot is detected by the caller's compiler, not by the repository"
    );
    change.rollback().await.unwrap();
    let unchanged = repository.load().await.unwrap();
    assert!(unchanged.groups[0].enabled);
    assert!(control_plane_audits(&repository).await.is_empty());
}

#[tokio::test]
async fn audit_failure_rolls_back_the_whole_prepared_change() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    execute(
        &database,
        &format!(
            "INSERT INTO api_keys
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
              allowed_group_ids,allowed_channel_ids)
             VALUES ('{KEY}','{USER}','User key','sk-secret','active',
                     '[\"open_ai_chat_completions\"]','[\"proxy\"]','[]','[]');"
        ),
    )
    .await;

    // `audit_logs.reason` is capped at 500 characters in the schema; the audit
    // insert fails at commit time and must take the revoke with it.
    let oversized = "x".repeat(501);
    let change = repository
        .prepare_own_api_key_revoke(USER, KEY, oversized)
        .await
        .unwrap();
    assert!(change.commit().await.is_err());
    let key = repository.own_api_key(USER, KEY).await.unwrap().unwrap();
    assert_eq!(key.status, "active");
    assert!(control_plane_audits(&repository).await.is_empty());

    // A bounded reason commits and writes exactly one self-service audit row.
    let change = repository
        .prepare_own_api_key_revoke(USER, KEY, "requested by the user".into())
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "self_revoke");
    let key = repository.own_api_key(USER, KEY).await.unwrap().unwrap();
    assert_eq!(key.status, "revoked");
    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].actor_role.as_deref(), Some("user"));
    assert_eq!(audits[0].reason.as_deref(), Some("requested by the user"));

    // The database rejects audit mutation, matching the PostgreSQL guard.
    let error = execute_expect_error(
        &database,
        "UPDATE audit_logs SET reason='tampered' WHERE object_id IS NOT NULL",
    )
    .await;
    assert!(
        error.is_some_and(|message| message.contains("audit_logs_immutable_update")),
        "audit rows are immutable"
    );
}

#[tokio::test]
async fn self_service_and_admin_writes_enforce_versions_and_policies() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();
    execute(
        &database,
        "INSERT INTO api_key_policies (id,name,allowed_group_ids,allowed_channel_ids,enabled)
         VALUES ('40200000-0000-0000-0000-000000000471','Default policy',
                 (
                   SELECT json_group_array(id) FROM channel_groups WHERE deleted_at IS NULL
                 ),
                 '[]',1)",
    )
    .await;
    execute(
        &database,
        &format!(
            "UPDATE user_groups SET default_api_key_policy_id='40200000-0000-0000-0000-000000000471',
                    updated_at=ag_now()
             WHERE id='{DEFAULT_USER_GROUP_ID}'"
        ),
    )
    .await;

    let options = repository.own_api_key_options(USER).await.unwrap();
    assert!(options.policy_enabled);
    assert_eq!(options.groups.len(), 1);
    assert_eq!(options.groups[0].api_format, "open_ai_chat_completions");
    assert_eq!(options.channels.len(), 1);

    let mut change = repository
        .prepare_own_api_key_create(
            USER,
            SelfApiKeyCreate {
                name: "Self key".into(),
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: Vec::new(),
                expires_at: None,
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
        )
        .await
        .unwrap();
    let records = change.runtime_records().await.unwrap();
    assert_eq!(records.control_plane.api_keys.len(), 1);
    let (mutations, _) = change.commit().await.unwrap();
    let created_id = mutations[0].id;
    let secret = mutations[0].created_secret.clone().unwrap();
    assert!(secret.starts_with("sk-"));
    let created = repository
        .own_api_key(USER, created_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        created.allowed_api_formats,
        vec!["open_ai_chat_completions"]
    );

    // A target outside the policy is rejected without writing anything.
    execute(
        &database,
        "INSERT INTO channel_groups (id,name,api_format,connector_kind,enabled)
         VALUES ('40300000-0000-0000-0000-0000000004a1','Outside Group',
                 'open_ai_chat_completions','openai_compatible',1)",
    )
    .await;
    assert!(matches!(
        repository
            .prepare_own_api_key_create(
                USER,
                SelfApiKeyCreate {
                    name: "Outside".into(),
                    allowed_group_ids: vec![Uuid::from_u128(0x4a1)],
                    allowed_channel_ids: Vec::new(),
                    expires_at: None,
                    requests_per_minute: None,
                    max_concurrent_requests: None,
                    quota_limit_amount: None,
                },
            )
            .await
            .err(),
        Some(RepositoryError::ApiKeyTargetNotAllowed)
    ));

    // A stale ETag conflicts; the fresh one updates.
    let updated_at = created.updated_at;
    let change = repository
        .prepare_own_api_key_update(
            USER,
            created_id,
            SelfApiKeyUpdate {
                name: "Renamed".into(),
                status: "disabled".into(),
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: Vec::new(),
                expires_at: None,
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
            updated_at,
        )
        .await
        .unwrap();
    change.rollback().await.unwrap();
    let stale = repository
        .prepare_own_api_key_update(
            USER,
            created_id,
            SelfApiKeyUpdate {
                name: "Renamed".into(),
                status: "disabled".into(),
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: Vec::new(),
                expires_at: None,
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
            updated_at - chrono::Duration::seconds(1),
        )
        .await;
    assert!(matches!(stale.err(), Some(RepositoryError::Conflict)));
    let change = repository
        .prepare_own_api_key_update(
            USER,
            created_id,
            SelfApiKeyUpdate {
                name: "Renamed".into(),
                status: "disabled".into(),
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: Vec::new(),
                expires_at: None,
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
            updated_at,
        )
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "self_update");

    // Only the owning user may delete; a mismatched owner is not found.
    let after = repository
        .own_api_key(USER, created_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        repository
            .prepare_own_api_key_delete(ADMIN, created_id, after.updated_at)
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));
    let change = repository
        .prepare_own_api_key_delete(USER, created_id, after.updated_at)
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "self_delete");
    assert!(
        repository
            .own_api_key(USER, created_id)
            .await
            .unwrap()
            .is_none(),
        "a deleted key is hidden from ordinary reads"
    );
    assert!(repository.own_api_keys(USER).await.unwrap().is_empty());
    assert_eq!(
        scalar::<String>(
            &database,
            "SELECT secret_value FROM api_keys WHERE name='Renamed'"
        )
        .await,
        format!("deleted-api-key-{created_id}"),
        "the deleted key's secret is erased in the database"
    );
    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits[0].action, "self_delete");
    assert_eq!(audits[0].object_type, "api_key");
}

#[tokio::test]
async fn user_settings_and_batches_apply_exactly_once() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;

    let (view, change) = repository
        .prepare_user_settings(
            USER,
            UserSettingsInput {
                websocket_enabled: true,
            },
        )
        .await
        .unwrap();
    assert!(view.websocket_enabled);
    change.commit().await.unwrap();
    assert!(
        repository
            .user_settings(USER)
            .await
            .unwrap()
            .unwrap()
            .websocket_enabled
    );
    assert!(
        control_plane_audits(&repository).await.is_empty(),
        "user settings changes are not audited"
    );
    assert!(matches!(
        repository
            .prepare_user_settings(
                Uuid::nil(),
                UserSettingsInput {
                    websocket_enabled: false
                }
            )
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));

    let user_before = repository
        .control_plane_lists()
        .await
        .unwrap()
        .users
        .into_iter()
        .find(|user| user.id == USER)
        .unwrap();
    let input: ai_gateway::persistence::UserBatchUpdateInput = serde_json::from_value(json!({
        "items": [{"id": USER, "updated_at": user_before.updated_at}],
        "changes": {"status": "suspended", "balance": {"operation": "increase", "amount": "2.5"}}
    }))
    .unwrap();
    let change = repository.prepare_users_batch(ADMIN, input).await.unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations.len(), 1);
    let after = repository
        .control_plane_lists()
        .await
        .unwrap()
        .users
        .into_iter()
        .find(|user| user.id == USER)
        .unwrap();
    assert_eq!(after.status, "suspended");
    assert_eq!(after.balance_amount.to_string(), "2.50000000");
    assert_eq!(
        control_plane_audits(&repository).await[0].action,
        "batch_update"
    );

    // A stale version conflicts and writes nothing further.
    let stale: ai_gateway::persistence::UserBatchUpdateInput = serde_json::from_value(json!({
        "items": [{"id": USER, "updated_at": user_before.updated_at}],
        "changes": {"status": "active"}
    }))
    .unwrap();
    assert!(matches!(
        repository.prepare_users_batch(ADMIN, stale).await.err(),
        Some(RepositoryError::Conflict)
    ));

    let channel_before = repository
        .control_plane_lists()
        .await
        .unwrap()
        .channels
        .into_iter()
        .find(|channel| channel.id == CHANNEL);
    if let Some(channel) = channel_before {
        let input: ai_gateway::persistence::ChannelBatchUpdateInput =
            serde_json::from_value(json!({
                "items": [{"id": channel.id, "updated_at": channel.updated_at}],
                "changes": {"enabled": false, "auto_disable_allowed": true}
            }))
            .unwrap();
        let change = repository
            .prepare_channels_batch(ADMIN, input)
            .await
            .unwrap();
        let (mutations, _) = change.commit().await.unwrap();
        assert_eq!(mutations[0].action, "batch_update");
        let after = repository
            .control_plane_lists()
            .await
            .unwrap()
            .channels
            .into_iter()
            .find(|channel| channel.id == CHANNEL)
            .unwrap();
        assert!(!after.enabled);
        assert!(after.auto_disable_allowed);
    }
}

#[tokio::test]
async fn catalog_catalog_models_import_and_refresh_existing_prices() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;

    let synced = |price: &str| ai_gateway::persistence::SyncedModelInput {
        source_model_id: "catalog-model".into(),
        display_name: "Catalog Model".into(),
        provider_name: "Provider".into(),
        input_unit_price: price.parse().unwrap(),
        cached_input_unit_price: "0".parse().unwrap(),
        cache_write_unit_price: "0".parse().unwrap(),
        output_unit_price: "0".parse().unwrap(),
        advanced_billing: serde_json::from_value(json!({
            "long_context_tiers": [],
            "request_multipliers": []
        }))
        .unwrap(),
        source_payload: json!({"id": "catalog-model"}),
    };

    let change = repository
        .prepare_catalog_models(ADMIN, vec![synced("1.5")])
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "import");
    let loaded = repository.load().await.unwrap();
    assert_eq!(loaded.models.len(), 1);
    assert_eq!(loaded.models[0].input_unit_price, Decimal::new(15, 1));

    let change = repository
        .prepare_catalog_models(ADMIN, vec![synced("2.75")])
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "price_sync");
    assert_eq!(mutations[0].id, loaded.models[0].id);
    let refreshed = repository.load().await.unwrap();
    assert_eq!(refreshed.models[0].input_unit_price, Decimal::new(275, 2));
    assert_eq!(
        repository.model_source_ids().await.unwrap(),
        vec!["catalog-model"]
    );
}

#[tokio::test]
async fn channel_auto_disable_and_recovery_follow_persisted_settings() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    execute(
        &database,
        "UPDATE channels SET auto_disable_allowed=1,updated_at=ag_now() WHERE id IS NOT NULL",
    )
    .await;
    repository.ensure_system_settings(settings()).await.unwrap();

    // The default settings do not match the trigger, so nothing is prepared.
    let trigger = ai_gateway::domain::AutomaticDisableTrigger::HttpStatus(503);
    assert!(
        repository
            .prepare_channel_disable(CHANNEL, &trigger)
            .await
            .unwrap()
            .is_none()
    );

    let mut settings_value = json!({
        "api_hosts": [],
        "upstream": {
            "connect_timeout_seconds": 10,
            "response_header_timeout_seconds": 30,
            "stream_idle_timeout_seconds": 60
        },
        "passive_health": {"connection_failure_threshold": 3, "cooldown_seconds": 60},
        "automatic_disable": {"enabled": true, "error_status_codes": [503]},
        "session_affinity": {"enabled": false, "max_entries": 100000, "default_ttl_seconds": 3600, "rules": []},
        "codex": {"originator": "codex_cli_rs", "client_version": "0.1.0", "user_agent": "codex_cli_rs/0.1.0"}
    });
    settings_value["api_hosts"] = json!([]);
    let input: SystemSettingsInput = serde_json::from_value(settings_value).unwrap();
    let change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::UpdateSystemSettings {
                input,
                expected_updated_at: repository.system_settings().await.unwrap().updated_at,
            },
        )
        .await
        .unwrap();
    change.commit().await.unwrap();

    let mut change = repository
        .prepare_channel_disable(CHANNEL, &trigger)
        .await
        .unwrap()
        .unwrap();
    assert!(
        change
            .runtime_records()
            .await
            .unwrap()
            .control_plane
            .channels[0]
            .auto_disabled
    );
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "auto_disable");
    assert_eq!(mutations[0].object_type, "channel");
    assert!(mutations[0].reason.as_deref().unwrap().contains("503"));
    let audits = control_plane_audits(&repository).await;
    assert_eq!(audits[0].actor_type, "system");
    assert_eq!(audits[0].action, "auto_disable");

    // A repeated trigger is idempotent because the channel is already disabled.
    assert!(
        repository
            .prepare_channel_disable(CHANNEL, &trigger)
            .await
            .unwrap()
            .is_none()
    );

    let change = repository
        .prepare_channel_recovery(CHANNEL)
        .await
        .unwrap()
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "auto_recover");
    let channel = repository
        .control_plane_lists()
        .await
        .unwrap()
        .channels
        .into_iter()
        .find(|channel| channel.id == CHANNEL)
        .unwrap();
    assert!(!channel.auto_disabled);
    assert!(
        repository
            .prepare_channel_recovery(CHANNEL)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn provider_managed_codex_resources_are_isolated_from_ordinary_management() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();
    // A Codex pool is created through the ordinary group mutation, which binds
    // an explicit connector pool for the Responses projection.
    execute(
        &database,
        "INSERT INTO connector_pools (id,connector_kind)
         VALUES ('40300000-0000-0000-0000-000000000481','codex_oauth')",
    )
    .await;

    let mut change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::CreateGroup(ChannelGroupInput {
                name: "Codex pool".into(),
                api_format: "open_ai_responses".into(),
                connector_kind: "codex_oauth".into(),
                request_compression: None,
                sharing_only: None,
                enabled: true,
                status_statistics_enabled: None,
            }),
        )
        .await
        .unwrap();
    let records = change.runtime_records().await.unwrap();
    let group = &records.control_plane.groups[0];
    assert_eq!(group.connector_kind, "codex_oauth");
    compile_runtime_config(records).unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    let responses_group = mutations[0].id;
    let pool_id = scalar::<String>(
        &database,
        "SELECT connector_pool_id FROM channel_groups \
         WHERE connector_kind='codex_oauth' AND api_format='open_ai_responses'",
    )
    .await;
    assert!(
        control_plane_audits(&repository).await[0]
            .after_redacted
            .as_ref()
            .unwrap()["connector_pool_groups"]
            .is_array(),
        "Codex group audits include the sibling projections"
    );
    assert_eq!(
        scalar::<String>(
            &database,
            "SELECT connector_pool_id FROM channel_groups WHERE id='{}'"
                .replace("{}", &responses_group.to_string())
                .as_str()
        )
        .await,
        pool_id
    );

    // An administrator cannot attach an ordinary channel to a Codex group.
    assert!(matches!(
        repository
            .prepare_mutation(
                ADMIN,
                ControlPlaneMutation::CreateChannel(ai_gateway::persistence::ChannelCreateInput {
                    channel_group_id: responses_group,
                    api_format: "open_ai_responses".into(),
                    name: "Ordinary in Codex".into(),
                    base_url: "https://upstream.example.test".into(),
                    enabled: true,
                    supports_websocket: false,
                    supports_standalone_web_search: false,
                    auto_disable_allowed: false,
                    billing_multiplier: Decimal::ONE,
                    proxy_id: None,
                    config_template_id: None,
                    override_document: json!({}),
                    connect_timeout_ms: None,
                    response_header_timeout_ms: None,
                    stream_idle_timeout_ms: None,
                    upstream_auth_kind: "none".into(),
                    upstream_auth_header_name: None,
                    upstream_api_key: None,
                    available_models: vec!["gpt-5".into()],
                    test_model: None,
                    test_pricing_model_id: None,
                },),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));

    // Images projections are hidden from ordinary listing and detail reads,
    // matching the schema's derived Images group and channel.
    let lists = repository.control_plane_lists().await.unwrap();
    assert_eq!(lists.channel_groups.len(), 2);
    assert!(lists.channels.is_empty());
    let images_group = lists
        .channel_groups
        .iter()
        .find(|group| group.id != responses_group)
        .unwrap();
    assert_eq!(images_group.api_format, "open_ai_images");
    assert!(
        !repository
            .channel_group_deletion_impact(responses_group)
            .await
            .is_ok()
    );
    assert!(matches!(
        repository
            .channel_group_deletion_impact(responses_group)
            .await
            .err(),
        Some(RepositoryError::ProviderManagedResource)
    ));
}

#[tokio::test]
async fn channel_routing_rules_are_replaced_whole_with_version_checks() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    seed_routing(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();

    // Replacing the tiers with a ready single-candidate graph commits in one step.
    let input: ai_gateway::persistence::ModelProtocolRuleInput = serde_json::from_value(json!({
        "description": "primary route",
        "enabled": true,
        "routing_tiers": [{
            "priority": 0,
            "selection_strategy": "weighted_round_robin",
            "candidates": [
                {"channel_id": CHANNEL, "upstream_model": "gpt-test", "weight": 2}
            ]
        }]
    }))
    .unwrap();
    let mut change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::UpdateProtocolRule {
                model_rule_id: PROFILE,
                id: RULE,
                input,
                expected_updated_at: rule_updated_at(&database).await,
            },
        )
        .await
        .unwrap();
    let records = change.runtime_records().await.unwrap();
    let tier = &records.control_plane.model_rules[0].routing_tiers[0];
    assert_eq!(tier.selection_strategy, "weighted_round_robin");
    assert_eq!(tier.candidates[0].weight, 2);
    compile_runtime_config(records).unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].action, "update");
    assert!(mutations[0].after_redacted["routing_tiers"].is_array());

    // A stale version is rejected before any tier is touched.
    let stale = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::UpdateProtocolRule {
                model_rule_id: PROFILE,
                id: RULE,
                input: serde_json::from_value(json!({
                    "description": null,
                    "enabled": false,
                    "routing_tiers": []
                }))
                .unwrap(),
                expected_updated_at: rule_updated_at(&database).await
                    - chrono::Duration::seconds(1),
            },
        )
        .await;
    assert!(matches!(stale.err(), Some(RepositoryError::Conflict)));

    // A candidate that no channel advertises fails closed as a routing dependency.
    let missing = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::UpdateProtocolRule {
                model_rule_id: PROFILE,
                id: RULE,
                input: serde_json::from_value(json!({
                    "description": null,
                    "enabled": true,
                    "routing_tiers": [{
                        "priority": 0,
                        "selection_strategy": "weighted_random",
                        "candidates": [
                            {"channel_id": CHANNEL, "upstream_model": "unknown-model", "weight": 1}
                        ]
                    }]
                }))
                .unwrap(),
                expected_updated_at: rule_updated_at(&database).await,
            },
        )
        .await;
    assert!(matches!(
        missing.err(),
        Some(RepositoryError::RoutingDependencyInvalid)
    ));
    let preserved = repository.load().await.unwrap().model_rules[0].routing_tiers[0].candidates[0]
        .upstream_model
        .clone();
    assert_eq!(preserved, "gpt-test");
}

#[tokio::test]
async fn invalid_management_inputs_fail_before_writing() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;

    assert!(matches!(
        repository
            .prepare_mutation(
                ADMIN,
                ControlPlaneMutation::CreateGroup(ChannelGroupInput {
                    name: "Bad format".into(),
                    api_format: "not-a-format".into(),
                    connector_kind: "openai_compatible".into(),
                    request_compression: None,
                    sharing_only: None,
                    enabled: true,
                    status_statistics_enabled: None,
                }),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                ADMIN,
                ControlPlaneMutation::CreateApiKeyPolicy(
                    ai_gateway::persistence::ApiKeyPolicyInput {
                        name: "Unbound policy".into(),
                        allowed_group_ids: vec![GROUP],
                        allowed_channel_ids: Vec::new(),
                        enabled: true,
                    },
                ),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(repository.load().await.unwrap().groups.is_empty());
    assert!(control_plane_audits(&repository).await.is_empty());

    // SQLite's own guards still reject direct invalid writes.
    assert!(
        execute_expect_error(
            &database,
            "INSERT INTO channel_groups (id,name,api_format)
             VALUES ('40300000-0000-0000-0000-000000000499','Bad','not-a-format')",
        )
        .await
        .is_some()
    );
}

/// Seeds one Codex connector pool and its Responses credential channel. The schema trigger
/// derives the paired Images group and channel plus both `codex_oauth_credential_channels`
/// projections.
async fn seed_codex_credential(
    database: &SqliteDatabase,
    pool: Uuid,
    responses_group: Uuid,
    credential: Uuid,
    label: &str,
    account_id: &str,
    provider_user_id: &str,
) {
    execute(
        database,
        &format!(
            "INSERT INTO connector_pools (id,connector_kind) VALUES ('{pool}','codex_oauth');
             INSERT INTO channel_groups
             (id,name,api_format,connector_kind,connector_pool_id,sharing_only)
             VALUES ('{responses_group}','{label} car','open_ai_responses','codex_oauth','{pool}',0);
             INSERT INTO channels
             (id,channel_group_id,api_format,name,base_url,upstream_auth_kind,supports_websocket)
             VALUES ('{credential}','{responses_group}','open_ai_responses','{label}',
                     'https://codex.invalid','none',1);
             INSERT INTO codex_oauth_credentials
             (channel_id,channel_group_id,connector_pool_id,label,account_id,user_id,id_token,
              access_token,refresh_token,last_refreshed_at)
             VALUES ('{credential}','{responses_group}','{pool}','{label}','{account_id}',
                     '{provider_user_id}','id','access','refresh',ag_now());",
        ),
    )
    .await;
}

async fn projected_channels(database: &SqliteDatabase, credential: Uuid) -> Vec<Uuid> {
    let mut reader = database.acquire_read().await.unwrap();
    sqlx::query_scalar::<_, SqliteUuid>(
        "SELECT channel_id FROM codex_oauth_credential_channels WHERE credential_id=?",
    )
    .bind(SqliteUuid(credential))
    .fetch_all(&mut *reader)
    .await
    .unwrap()
    .into_iter()
    .map(|value| value.0)
    .collect()
}

async fn projected_format_channel(
    database: &SqliteDatabase,
    credential: Uuid,
    api_format: &str,
) -> Uuid {
    let mut reader = database.acquire_read().await.unwrap();
    sqlx::query_scalar::<_, SqliteUuid>(
        "SELECT channel_id FROM codex_oauth_credential_channels \
         WHERE credential_id=? AND api_format=?",
    )
    .bind(SqliteUuid(credential))
    .bind(api_format)
    .fetch_one(&mut *reader)
    .await
    .unwrap()
    .0
}

fn sorted(mut values: Vec<Uuid>) -> Vec<Uuid> {
    values.sort_unstable();
    values.dedup();
    values
}

/// Sharing associations must be keyed by the group's `credential_id`, not by the group's own id.
/// The fixture gives the group an id distinct from its credential and adds a recognizable
/// provider-identity alias in another pool, so a mis-keyed lookup returns empty channel/window
/// lists and the alias loses its protection even though no group is `sharing_only`.
#[tokio::test]
async fn sharing_projections_follow_the_credential_not_the_group_identity() {
    let (_directory, database, repository) = repository().await;
    seed_admin_and_user(&database).await;
    repository.ensure_system_settings(settings()).await.unwrap();

    let pool = Uuid::from_u128(0x4b1);
    let responses_group = Uuid::from_u128(0x4b2);
    let sharing_group = Uuid::from_u128(0x4b3);
    let credential = Uuid::from_u128(0x4b4);
    let identity_alias = Uuid::from_u128(0x4b5);
    let primary_window = Uuid::from_u128(0x4b6);
    let secondary_window = Uuid::from_u128(0x4b7);
    let alias_pool = Uuid::from_u128(0x4c1);
    let alias_group = Uuid::from_u128(0x4c2);
    let outsider = Uuid::from_u128(0x4c3);

    seed_codex_credential(
        &database,
        pool,
        responses_group,
        credential,
        "Canonical",
        "shared-account",
        "shared-user",
    )
    .await;
    seed_codex_credential(
        &database,
        alias_pool,
        alias_group,
        identity_alias,
        "Alias",
        "shared-account",
        "shared-user",
    )
    .await;
    execute(
        &database,
        &format!(
            "UPDATE codex_oauth_credentials SET updated_at=ag_now(),\
                 primary_used_percent=10,primary_window_seconds=300,\
                 primary_reset_at='2099-01-01T00:00:00.000000Z',\
                 secondary_used_percent=5,secondary_window_seconds=3600,\
                 secondary_reset_at='2099-01-01T00:00:00.000005Z',\
                 quota_checked_at='2099-01-01T00:00:00.000000Z' \
             WHERE channel_id='{credential}';
             INSERT INTO codex_sharing_groups
             (id,credential_id,provider_account_id,provider_user_id,name,enabled,seats,
              primary_limit_amount,secondary_limit_amount,request_reservation_amount,
              user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests,
              group_max_concurrent_requests)
             VALUES ('{sharing_group}','{credential}','shared-account','shared-user','Car',1,
                     '[\"{ADMIN}\",\"{USER}\"]','10','10','0.01',10,10,1,1);
             INSERT INTO codex_quota_window_periods
             (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at,
              initial_used_percent,last_used_percent,first_observed_at,last_observed_at)
             VALUES ('{primary_window}','{credential}','primary',300,
                     '2098-12-31T00:00:00.000000Z','2099-01-01T00:00:00.000000Z',10,10,
                     '2098-12-31T00:00:00.000000Z','2099-01-01T00:00:00.000000Z');
             INSERT INTO codex_quota_window_periods
             (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at,
              initial_used_percent,last_used_percent,first_observed_at,last_observed_at)
             VALUES ('{secondary_window}','{credential}','secondary',3600,
                     '2098-12-31T00:00:00.000000Z','2099-01-01T00:00:00.000005Z',5,5,
                     '2098-12-31T00:00:00.000000Z','2099-01-01T00:00:00.000000Z');"
        ),
    )
    .await;

    let records = repository.load_runtime().await.unwrap();
    assert!(
        records.sharing_only_channels.is_empty(),
        "no group is sharing-only"
    );
    assert_eq!(records.sharing.len(), 1);
    let record = &records.sharing[0];
    assert_eq!(record.group.id, sharing_group);
    assert_eq!(record.group.policy.credential_id, credential);

    let credential_images = projected_format_channel(&database, credential, "open_ai_images").await;
    let alias_images = projected_format_channel(&database, identity_alias, "open_ai_images").await;
    assert_eq!(
        sorted(record.channel_ids.clone()),
        sorted(projected_channels(&database, credential).await),
        "channel projections are keyed by credential, not by the sharing group id"
    );
    assert_eq!(
        sorted(record.channel_ids.clone()),
        sorted(vec![credential, credential_images])
    );
    assert_eq!(
        sorted(record.protected_channel_ids.clone()),
        sorted(vec![
            credential,
            credential_images,
            identity_alias,
            alias_images
        ]),
        "protected identities cover both the canonical credential and the provider alias"
    );
    assert!(
        !record.protected_channel_ids.contains(&sharing_group),
        "the sharing group id is not a channel"
    );

    let mut windows = record.windows.clone();
    windows.sort_by_key(|window| window.window_kind.clone());
    assert_eq!(windows.len(), 2);
    assert!(
        windows
            .iter()
            .all(|window| window.credential_id == credential),
        "windows are keyed by credential"
    );
    assert_eq!(windows[0].window_kind, "primary");
    assert_eq!(windows[0].id, primary_window);
    assert_eq!(windows[0].used_percent, 10);
    assert_eq!(windows[1].window_kind, "secondary");
    assert_eq!(windows[1].id, secondary_window);
    assert_eq!(windows[1].used_percent, 5);

    // The compiled registry keeps the same protections: the group is not `sharing_only`, yet an
    // unseated user is still denied the alias channel, the member reaches only the canonical
    // projections, and the alias itself is never an eligible canonical channel.
    let compiled = compile_runtime_config(records).unwrap();
    let sharing = compiled.sharing();
    assert_eq!(
        sharing.for_user(ADMIN).map(|group| group.id),
        Some(sharing_group)
    );
    assert!(sharing.for_user(outsider).is_none());
    for channel in [credential, credential_images] {
        assert_eq!(
            sharing.for_channel(channel).map(|group| group.id),
            Some(sharing_group)
        );
    }
    assert!(sharing.permits(ADMIN, credential));
    assert!(sharing.permits(ADMIN, credential_images));
    for channel in [credential, credential_images, identity_alias, alias_images] {
        assert!(
            !sharing.permits(outsider, channel),
            "an unseated user is denied {channel}"
        );
    }
    assert!(sharing.is_protected(identity_alias));
    assert!(sharing.is_protected(alias_images));
    assert!(
        !sharing.permits(ADMIN, identity_alias),
        "an identity alias is protected but not canonical"
    );
}
