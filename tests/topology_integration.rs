//! Current topology, grant lifecycle, and reporting against disposable databases.
#![cfg(target_os = "linux")]

use ai_gateway::persistence::{ControlPlaneRepository, UpstreamTopologyRecords};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[path = "support/postgres.rs"]
mod postgres;

const GROUP: Uuid = Uuid::from_u128(0x4000_0000_0000_0000_0000_0000_0000_0001);
const CHANNEL: Uuid = Uuid::from_u128(0x5000_0000_0000_0000_0000_0000_0000_0001);
const MODEL: Uuid = Uuid::from_u128(0x3000_0000_0000_0000_0000_0000_0000_0001);
const PROFILE: Uuid = Uuid::from_u128(0x6000_0000_0000_0000_0000_0000_0000_0001);
const RULE: Uuid = Uuid::from_u128(0x7000_0000_0000_0000_0000_0000_0000_0001);
const USER: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0001);
const KEY: Uuid = Uuid::from_u128(0x2000_0000_0000_0000_0000_0000_0000_0001);
const POLICY: Uuid = Uuid::from_u128(0x9000_0000_0000_0000_0000_0000_0000_0001);
const ACCESS: Uuid = Uuid::from_u128(401);
const CAPABILITY: Uuid = Uuid::from_u128(501);
const TIER: Uuid = Uuid::from_u128(601);

fn sorted<T: Serialize>(values: &[T]) -> Vec<Value> {
    let mut values = values
        .iter()
        .map(|value| serde_json::to_value(value).expect("canonical record is serializable"))
        .collect::<Vec<_>>();
    values.sort_by_key(|value| value.to_string());
    values
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
        let mut database_url = postgres::admin_url(None).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url.as_str())
            .await
            .expect("configured PostgreSQL administrator database must be available");
        let name = format!("ai_gateway_topology_{}", Uuid::new_v4().simple());
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
        ai_gateway::persistence::run_migrations(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(seed_sql(false)))
            .execute(&pool)
            .await
            .unwrap();
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

fn seed_sql(sqlite: bool) -> String {
    let array = |value: &str| {
        if sqlite {
            format!("'[\"{value}\"]'")
        } else {
            format!("'{{{value}}}'")
        }
    };
    let groups = array(&GROUP.to_string());
    let channels = array(&CHANNEL.to_string());
    let direct_key = Uuid::from_u128(202);
    let permissions = array("proxy");
    let models = array("wire-model");
    format!(
        "INSERT INTO api_key_policies(id,name,allowed_group_ids,allowed_channel_ids) VALUES ('{POLICY}','Members',{groups},{channels});
         INSERT INTO users(id,display_name,email,role,status,password_hash,default_api_key_policy_id)
         VALUES ('{USER}','Test admin','admin@example.test','admin','active','test-hash','{POLICY}');
         INSERT INTO api_keys(id,user_id,name,secret_value,status,permissions,allowed_group_ids)
         VALUES ('{KEY}','{USER}','Group key','group-key-secret','active',{permissions},{groups});
         INSERT INTO models(id,source_model_id,display_name,price_unit_tokens,input_unit_price,
             cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
         VALUES ('{MODEL}','client-model','Client model',1000000,'1','0','0','2','2026-01-01T00:00:00.000000Z');
         INSERT INTO model_routing_profiles(id,model_id) VALUES ('{PROFILE}','{MODEL}');
         INSERT INTO routing_groups(id,name) VALUES ('{GROUP}','Primary group');
         INSERT INTO upstream_accesses(id,name,connector_kind,base_url)
         VALUES ('{ACCESS}','Test access','general','https://upstream.test');
         INSERT INTO upstream_channels(id,name,group_id,access_id) VALUES ('{CHANNEL}','Primary channel','{GROUP}','{ACCESS}');
         INSERT INTO api_keys(id,user_id,name,secret_value,status,permissions,allowed_channel_ids)
         VALUES ('{direct_key}','{USER}','Direct key','direct-key-secret','active',{permissions},{channels});
         INSERT INTO api_key_channel_grants(api_key_id,channel_id) VALUES ('{direct_key}','{CHANNEL}');
         INSERT INTO api_key_policy_channel_grants(policy_id,channel_id) VALUES ('{POLICY}','{CHANNEL}');
         INSERT INTO channel_capabilities(id,channel_id,operation,enabled,available_models,test_model,test_pricing_model_id)
         VALUES ('{CAPABILITY}','{CHANNEL}','chat_completion',true,{models},'wire-model','{MODEL}');
         INSERT INTO model_operation_rules(id,model_routing_profile_id,operation) VALUES ('{RULE}','{PROFILE}','chat_completion');
         INSERT INTO model_capability_tiers(id,rule_id,operation,priority,strategy)
         VALUES ('{TIER}','{RULE}','chat_completion',0,'weighted_round_robin');
         INSERT INTO model_capability_candidates(tier_id,operation,capability_id,upstream_model,weight)
         VALUES ('{TIER}','chat_completion','{CAPABILITY}','wire-model',5);
         INSERT INTO group_identity_registry(id,label,created_at,canonical_group_id) SELECT id,name,created_at,id FROM routing_groups;
         INSERT INTO channel_identity_registry(id,label,created_at,canonical_channel_id) SELECT id,name,created_at,id FROM upstream_channels;
         INSERT INTO channel_identity_registry(id,label,created_at,canonical_channel_id,capability_id)
         SELECT cap.id,c.name,cap.created_at,c.id,cap.id FROM channel_capabilities cap JOIN upstream_channels c ON c.id=cap.channel_id;
         INSERT INTO model_rule_identity_registry(id,label,created_at,canonical_rule_id)
         SELECT id,'Chat',created_at,id FROM model_operation_rules;"
    )
}

async fn exercise_authorization(repository: ai_gateway::persistence::ControlPlaneRepository) {
    use ai_gateway::domain::{ApiOperation, CapabilitySettings, RequestCompression};
    use ai_gateway::persistence::{
        ApiKeyCreate, ApiKeyPolicyInput, ApiKeyUpdate, ChannelCapabilityInput,
        ControlPlaneMutation, SelfApiKeyCreate,
    };
    let key = repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::CreateApiKey(ApiKeyCreate {
                user_id: USER,
                name: "Group target key".into(),
                expires_at: None,
                permissions: vec!["proxy".into()],
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            }),
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    let images = Uuid::new_v4();
    repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::SaveChannelCapability {
                id: images,
                input: ChannelCapabilityInput {
                    channel_id: CHANNEL,
                    settings: CapabilitySettings {
                        operation: ApiOperation::ImagesGeneration,
                        enabled: false,
                        available_models: vec!["image-model".into()],
                        request_compression: RequestCompression::Default,
                        test_model: None,
                        test_pricing_model_id: None,
                        auto_disable_allowed: false,
                    },
                    status_statistics_enabled: false,
                    config_template_id: None,
                    override_document: serde_json::json!({}),
                    billing_multiplier: rust_decimal::Decimal::ONE,
                },
                expected: None,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let options = repository.own_api_key_options(USER).await.unwrap();
    assert_eq!(
        options.groups[0].api_formats,
        ["open_ai_chat_completions", "open_ai_images"]
    );
    let policy = repository
        .admin_api_key_policies()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == POLICY)
        .unwrap();
    repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::UpdateApiKeyPolicy {
                id: POLICY,
                expected_updated_at: policy.updated_at,
                input: ApiKeyPolicyInput {
                    name: "Renamed group policy".into(),
                    enabled: true,
                    allowed_group_ids: vec![GROUP],
                    allowed_channel_ids: vec![],
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let self_key = repository
        .prepare_own_api_key_create(
            USER,
            SelfApiKeyCreate {
                name: "Policy bounded key".into(),
                expires_at: None,
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    let update = |channels| ApiKeyUpdate {
        name: "Renamed group key".into(),
        status: "active".into(),
        expires_at: None,
        permissions: vec!["proxy".into()],
        allowed_group_ids: vec![GROUP],
        allowed_channel_ids: channels,
        requests_per_minute: Some(50),
        max_concurrent_requests: None,
        quota_limit_amount: None,
    };
    let renamed = repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::UpdateApiKey {
                id: key.id,
                input: update(vec![]),
                expected_updated_at: key.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    assert!(
        repository
            .prepare_mutation(
                USER,
                ControlPlaneMutation::UpdateApiKey {
                    id: key.id,
                    input: update(vec![CHANNEL]),
                    expected_updated_at: key.updated_at,
                }
            )
            .await
            .is_err()
    );
    repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::UpdateApiKey {
                id: key.id,
                input: update(vec![CHANNEL]),
                expected_updated_at: renamed.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    exercise_self_service_channel_boundary(&repository, images).await;
    exercise_capability_batch_and_recovery(&repository).await;
    exercise_channel_authority_and_route_withdrawal(
        &repository,
        key.id,
        key.created_secret.as_deref().unwrap(),
        self_key.created_secret.as_deref().unwrap(),
    )
    .await;
    exercise_model_lifecycle(&repository).await;
}

fn assert_responses_channels(
    snapshot: &ai_gateway::domain::CompiledRuntimeConfig,
    secret: &str,
    expected: &[Uuid],
) {
    use ai_gateway::domain::{ApiKeyPermission, ApiOperation, RequestProtocol};
    use ai_gateway::routing::{PassiveHealthPolicy, RoutingRuntime, SelectionResult};
    let routing = RoutingRuntime::new(PassiveHealthPolicy::default());
    let key = snapshot.authenticate(secret).unwrap();
    assert!(key.permits(ApiKeyPermission::Proxy));
    let mut excluded = Vec::new();
    let mut channels = std::collections::BTreeSet::new();
    while let SelectionResult::Selected(selected) = routing
        .select_operation_with_affinity_excluding(
            snapshot,
            &key,
            ApiOperation::Responses,
            RequestProtocol::NonStream,
            "client-model",
            None,
            &excluded,
        )
    {
        assert!(!excluded.contains(&selected.candidate_slot));
        excluded.push(selected.candidate_slot);
        channels.insert(selected.channel.id());
    }
    assert_eq!(channels, expected.iter().copied().collect());
}

async fn exercise_key_revocation(
    repository: &ai_gateway::persistence::ControlPlaneRepository,
    coordinator: &ai_gateway::application::ControlPlaneCoordinator,
    runtime: &ai_gateway::runtime_config::RuntimeConfig,
    direct_capability: Uuid,
    group_capability: Uuid,
) {
    use ai_gateway::persistence::{ApiKeyUpdate, ControlPlaneMutation};
    let key = repository.own_api_key(USER, KEY).await.unwrap().unwrap();
    let update = |groups, channels, expected_updated_at| ControlPlaneMutation::UpdateApiKey {
        id: KEY,
        expected_updated_at,
        input: ApiKeyUpdate {
            name: key.name.clone(),
            status: "active".into(),
            expires_at: None,
            permissions: key.permissions.clone(),
            allowed_group_ids: groups,
            allowed_channel_ids: channels,
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
        },
    };
    let with_direct = coordinator
        .mutate(USER, update(vec![GROUP], vec![CHANNEL], key.updated_at))
        .await
        .unwrap();
    let retained = repository.topology().await.unwrap().api_key_grants;
    assert!(
        retained
            .iter()
            .any(|g| g.api_key_id == KEY && g.channel_id == CHANNEL)
    );
    let direct_only = coordinator
        .mutate(USER, update(vec![], vec![CHANNEL], with_direct.updated_at))
        .await
        .unwrap();
    assert_eq!(
        sorted(&repository.topology().await.unwrap().api_key_grants),
        sorted(&retained)
    );
    assert_responses_channels(
        &runtime.snapshot(),
        "group-key-secret",
        &[direct_capability],
    );

    coordinator
        .mutate(USER, update(vec![GROUP], vec![], direct_only.updated_at))
        .await
        .unwrap();
    assert_responses_channels(&runtime.snapshot(), "group-key-secret", &[group_capability]);
}

async fn exercise_channel_authority_and_route_withdrawal(
    repository: &ai_gateway::persistence::ControlPlaneRepository,
    key_id: Uuid,
    group_and_direct: &str,
    self_group: &str,
) {
    use ai_gateway::application::ControlPlaneCoordinator;
    use ai_gateway::domain::{ApiOperation, CapabilityTransport};
    use ai_gateway::persistence::{
        ApiKeyPolicyInput, ControlPlaneMutation as M, LogicalChannelInput, RepositoryError,
        RoutingGroupInput, SelfApiKeyCreate, SelfApiKeyUpdate,
    };
    use ai_gateway::routing::{PassiveHealthPolicy, RoutingRuntime};
    use ai_gateway::runtime_config::{RuntimeConfig, compile_runtime_config};
    use std::sync::Arc;

    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        runtime.clone(),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let self_key = |name: &str, groups, channels| SelfApiKeyCreate {
        name: name.into(),
        expires_at: None,
        allowed_group_ids: groups,
        allowed_channel_ids: channels,
        requests_per_minute: None,
        max_concurrent_requests: None,
        quota_limit_amount: None,
    };
    let direct = coordinator
        .create_own_api_key(USER, self_key("Direct", vec![], vec![CHANNEL]))
        .await
        .unwrap();
    let direct_secret = direct.created_secret.as_deref().unwrap();
    let initial = repository.topology().await.unwrap();
    let original = initial
        .logical_channels
        .iter()
        .find(|channel| channel.id == CHANNEL)
        .unwrap();
    let access = original.access_id;
    let other = Uuid::new_v4();
    let own_cap = Uuid::new_v4();
    let other_cap = Uuid::new_v4();
    let rule_id = Uuid::new_v4();
    let create_capability = |id, channel, operation| M::SaveChannelCapability {
        id,
        expected: None,
        input: serde_json::from_value(serde_json::json!({
            "channel_id": channel,
            "settings": {
                "operation": operation, "enabled": true,
                "available_models": ["wire-model", "wire-alias"],
                "request_compression": "default", "test_model": null,
                "test_pricing_model_id": null, "auto_disable_allowed": false
            }
        }))
        .unwrap(),
    };
    for mutation in [
        M::SaveLogicalChannel {
            id: other, expected: None,
            input: serde_json::from_value(serde_json::json!({
                "group_id": GROUP, "access_id": access, "credential_id": null,
                "name": "Later group member", "enabled": true
            })).unwrap(),
        },
        create_capability(own_cap, CHANNEL, "responses"),
        create_capability(other_cap, other, "responses"),
        M::SaveOperationRule {
            id: rule_id, expected_updated_at: None,
            input: serde_json::from_value(serde_json::json!({
                "model_routing_profile_id": PROFILE, "operation": "responses", "enabled": true,
                "routing_tiers": [
                    {"priority": 0, "selection_strategy": "weighted_round_robin",
                     "candidates": [
                         {"capability_id": own_cap, "upstream_model": "wire-model", "weight": 2},
                         {"capability_id": other_cap, "upstream_model": "wire-model", "weight": 7}]},
                    {"priority": 9, "selection_strategy": "weighted_random",
                     "candidates": [{"capability_id": own_cap, "upstream_model": "wire-alias", "weight": 5}]}
                ]
            })).unwrap(),
        },
    ] {
        coordinator.mutate(USER, mutation).await.unwrap();
    }
    assert!(
        repository
            .own_api_key_options(USER)
            .await
            .unwrap()
            .channels
            .iter()
            .any(|channel| channel.id == other)
    );
    let snapshot = runtime.snapshot();
    let http = snapshot.channel(own_cap).unwrap();
    assert_eq!(http.logical_channel_id(), CHANNEL);
    assert_eq!(http.api_operation(), ApiOperation::Responses);
    assert!(http.permits_transport(CapabilityTransport::HttpJson));
    assert!(http.permits_transport(CapabilityTransport::HttpSse));
    assert!(!http.permits_transport(CapabilityTransport::Websocket));
    assert!(
        snapshot
            .operation_rule(ApiOperation::ResponsesWebSocket, "client-model")
            .is_none()
    );
    for secret in ["group-key-secret", self_group, group_and_direct] {
        assert_responses_channels(&snapshot, secret, &[own_cap, other_cap]);
    }
    assert_responses_channels(&snapshot, direct_secret, &[own_cap]);

    let empty_group = Uuid::new_v4();
    coordinator
        .mutate(
            USER,
            M::SaveRoutingGroup {
                id: empty_group,
                expected: None,
                input: RoutingGroupInput {
                    name: "Initially empty".into(),
                    enabled: true,
                },
            },
        )
        .await
        .unwrap();
    let policy = repository
        .admin_api_key_policies()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == POLICY)
        .unwrap();
    coordinator
        .mutate(
            USER,
            M::UpdateApiKeyPolicy {
                id: POLICY,
                expected_updated_at: policy.updated_at,
                input: ApiKeyPolicyInput {
                    name: "Empty group and one channel".into(),
                    enabled: true,
                    allowed_group_ids: vec![empty_group],
                    allowed_channel_ids: vec![CHANNEL],
                },
            },
        )
        .await
        .unwrap();
    let options = repository.own_api_key_options(USER).await.unwrap();
    assert_eq!(
        options.groups.iter().map(|g| g.id).collect::<Vec<_>>(),
        [empty_group]
    );
    assert_eq!(
        options.channels.iter().map(|c| c.id).collect::<Vec<_>>(),
        [CHANNEL]
    );
    assert!(matches!(
        repository
            .prepare_own_api_key_create(
                USER,
                self_key("Not a permitted group", vec![GROUP], vec![])
            )
            .await
            .err(),
        Some(RepositoryError::ApiKeyTargetNotAllowed)
    ));
    let upgrade = SelfApiKeyUpdate {
        name: "Widened scope".into(),
        status: "active".into(),
        expires_at: None,
        allowed_group_ids: vec![GROUP],
        allowed_channel_ids: vec![CHANNEL],
        requests_per_minute: None,
        max_concurrent_requests: None,
        quota_limit_amount: None,
    };
    assert!(matches!(
        repository
            .prepare_own_api_key_update(USER, direct.id, upgrade, direct.updated_at)
            .await
            .err(),
        Some(RepositoryError::ApiKeyTargetNotAllowed)
    ));
    let empty = coordinator
        .create_own_api_key(USER, self_key("Empty group", vec![empty_group], vec![]))
        .await
        .unwrap();
    let empty_secret = empty.created_secret.as_deref().unwrap();
    let before_move = runtime.snapshot();
    assert_responses_channels(&before_move, empty_secret, &[]);
    assert_responses_channels(&before_move, self_group, &[own_cap, other_cap]);
    coordinator
        .mutate(
            USER,
            M::SaveLogicalChannel {
                id: CHANNEL,
                expected: Some(original.updated_at),
                input: LogicalChannelInput {
                    group_id: empty_group,
                    access_id: access,
                    credential_id: original.credential_id,
                    name: original.name.clone(),
                    enabled: original.enabled,
                },
            },
        )
        .await
        .unwrap();
    let moved = runtime.snapshot();
    for secret in ["group-key-secret", self_group] {
        assert_responses_channels(&moved, secret, &[other_cap]);
    }
    assert_responses_channels(&moved, empty_secret, &[own_cap]);
    assert_responses_channels(&moved, direct_secret, &[own_cap]);
    assert_responses_channels(&moved, group_and_direct, &[own_cap, other_cap]);
    assert_responses_channels(&before_move, empty_secret, &[]);
    exercise_key_revocation(repository, &coordinator, &runtime, own_cap, other_cap).await;
    let before = repository.topology().await.unwrap();
    let own_version = before
        .channel_capabilities
        .iter()
        .find(|capability| capability.id == own_cap)
        .unwrap()
        .updated_at;
    let pending = repository
        .prepare_mutation(
            USER,
            M::DeleteChannelCapability {
                id: own_cap,
                expected: own_version,
            },
        )
        .await
        .unwrap();
    drop(pending);
    assert_topology(&before, &repository.topology().await.unwrap());
    let mutation = coordinator
        .mutate(
            USER,
            M::DeleteChannelCapability {
                id: own_cap,
                expected: own_version,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        mutation.after_redacted["detached_routes"],
        serde_json::json!([
            {"rule_id": rule_id, "disabled": false}
        ])
    );
    let after = repository.topology().await.unwrap();
    let tiers = after
        .operation_tiers
        .iter()
        .filter(|tier| tier.rule_id == rule_id)
        .collect::<Vec<_>>();
    assert_eq!(tiers.len(), 1);
    assert_eq!(tiers[0].priority, 0);
    assert_eq!(tiers[0].strategy, "weighted_round_robin");
    let candidates = after
        .operation_candidates
        .iter()
        .filter(|candidate| candidate.tier_id == tiers[0].id)
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].capability_id, other_cap);
    assert_eq!(candidates[0].weight, 7);
    assert_eq!(candidates[0].upstream_model, "wire-model");
    assert!(
        after
            .operation_rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .unwrap()
            .enabled
    );
    assert_responses_channels(&runtime.snapshot(), group_and_direct, &[other_cap]);
    assert_responses_channels(&runtime.snapshot(), direct_secret, &[]);
    coordinator
        .mutate(
            USER,
            M::DeleteChannelCapability {
                id: other_cap,
                expected: after
                    .channel_capabilities
                    .iter()
                    .find(|capability| capability.id == other_cap)
                    .unwrap()
                    .updated_at,
            },
        )
        .await
        .unwrap();
    let after = repository.topology().await.unwrap();
    assert!(
        !after
            .operation_rules
            .iter()
            .find(|rule| rule.id == rule_id)
            .unwrap()
            .enabled
    );
    assert!(
        after
            .operation_tiers
            .iter()
            .all(|tier| tier.rule_id != rule_id)
    );
    coordinator
        .mutate(
            USER,
            M::DeleteLogicalChannel {
                id: other,
                expected: after
                    .logical_channels
                    .iter()
                    .find(|channel| channel.id == other)
                    .unwrap()
                    .updated_at,
            },
        )
        .await
        .unwrap();
    assert_responses_channels(&runtime.snapshot(), group_and_direct, &[]);
    let replacement = Uuid::new_v4();
    coordinator
        .mutate(
            USER,
            create_capability(replacement, CHANNEL, "responses-ws"),
        )
        .await
        .unwrap();
    let ws = runtime.snapshot().channel(replacement).unwrap();
    assert_eq!(ws.logical_channel_id(), CHANNEL);
    assert_eq!(ws.api_operation(), ApiOperation::ResponsesWebSocket);
    assert!(ws.permits_transport(CapabilityTransport::Websocket));
    assert!(!ws.permits_transport(CapabilityTransport::HttpJson));
    assert!(!ws.permits_transport(CapabilityTransport::HttpSse));
    assert!(
        repository
            .load()
            .await
            .unwrap()
            .api_keys
            .iter()
            .find(|key| key.id == key_id)
            .unwrap()
            .allowed_channel_ids
            .contains(&replacement)
    );
}

async fn exercise_capability_batch_and_recovery(
    repository: &ai_gateway::persistence::ControlPlaneRepository,
) {
    use ai_gateway::domain::{ApiOperation, AutomaticDisableTrigger};
    use ai_gateway::persistence::{
        ChannelBatchChanges, ChannelBatchUpdateInput, ChannelBatchUpdateTarget,
        ControlPlaneMutation, RepositoryError,
    };
    repository.ensure_system_settings(serde_json::from_value(serde_json::json!({
        "api_hosts": [],
        "upstream": {"connect_timeout_seconds": 10, "response_header_timeout_seconds": 30,
                     "stream_idle_timeout_seconds": 60},
        "passive_health": {"connection_failure_threshold": 3, "cooldown_seconds": 60},
        "automatic_disable": {"enabled": true, "error_status_codes": [429]},
        "session_affinity": {"enabled": false, "max_entries": 100000, "default_ttl_seconds": 3600, "rules": []}
    })).unwrap()).await.unwrap();
    exercise_catalog_append(repository).await;
    let before = repository.topology().await.unwrap();
    let capability = before
        .channel_capabilities
        .iter()
        .find(|capability| {
            capability.channel_id == CHANNEL
                && capability.settings.operation == ApiOperation::ChatCompletions
        })
        .unwrap();
    let target = ChannelBatchUpdateTarget {
        id: capability.id,
        updated_at: capability.updated_at,
    };
    let changes = ChannelBatchChanges {
        enabled: Some(true),
        auto_disable_allowed: Some(true),
        billing_multiplier: Some(rust_decimal::Decimal::from(2)),
        append_model: None,
    };
    let invalid = repository
        .prepare_channels_batch(
            USER,
            ChannelBatchUpdateInput {
                items: vec![
                    target.clone(),
                    ChannelBatchUpdateTarget {
                        id: Uuid::new_v4(),
                        updated_at: capability.updated_at,
                    },
                ],
                changes: changes.clone(),
            },
        )
        .await;
    assert!(matches!(invalid.err(), Some(RepositoryError::NotFound)));
    assert_topology(&before, &repository.topology().await.unwrap());
    repository
        .prepare_channels_batch(
            USER,
            ChannelBatchUpdateInput {
                items: vec![target.clone()],
                changes: changes.clone(),
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        repository
            .prepare_channels_batch(
                USER,
                ChannelBatchUpdateInput {
                    items: vec![target],
                    changes,
                }
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let trigger = AutomaticDisableTrigger::HttpStatus(429);
    repository
        .prepare_channel_disable(capability.id, &trigger)
        .await
        .unwrap()
        .unwrap()
        .commit()
        .await
        .unwrap();
    let disabled = repository
        .topology()
        .await
        .unwrap()
        .channel_capabilities
        .into_iter()
        .find(|row| row.id == capability.id)
        .unwrap();
    assert!(disabled.auto_disabled);
    repository
        .prepare_channels_batch(
            USER,
            ChannelBatchUpdateInput {
                items: vec![ChannelBatchUpdateTarget {
                    id: capability.id,
                    updated_at: disabled.updated_at,
                }],
                changes: ChannelBatchChanges {
                    enabled: Some(false),
                    auto_disable_allowed: None,
                    billing_multiplier: None,
                    append_model: None,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        repository
            .prepare_mutation(
                USER,
                ControlPlaneMutation::RecoverChannel {
                    id: capability.id,
                    expected_updated_at: disabled.updated_at,
                }
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let current = repository
        .topology()
        .await
        .unwrap()
        .channel_capabilities
        .into_iter()
        .find(|row| row.id == capability.id)
        .unwrap();
    let recovered = repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::RecoverChannel {
                id: capability.id,
                expected_updated_at: current.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    assert_eq!(recovered.object_type, "channel_capability");
    assert_eq!(recovered.action, "manual_recover");
    let after = repository.topology().await.unwrap();
    let recovered = after
        .channel_capabilities
        .iter()
        .find(|row| row.id == capability.id)
        .unwrap();
    assert!(!recovered.auto_disabled);
    assert!(!recovered.settings.enabled);
    assert!(recovered.auto_disable_reason.is_none());
    assert!(recovered.auto_disable_at.is_none());
    assert_ne!(recovered.revision, current.revision);
    assert_eq!(
        sorted(&before.logical_channels),
        sorted(&after.logical_channels)
    );
    assert_eq!(
        sorted(&before.api_key_grants),
        sorted(&after.api_key_grants)
    );
    assert_eq!(sorted(&before.policy_grants), sorted(&after.policy_grants));
    let unchanged_before = before
        .channel_capabilities
        .into_iter()
        .filter(|row| row.id != recovered.id)
        .collect::<Vec<_>>();
    let unchanged_after = after
        .channel_capabilities
        .iter()
        .filter(|row| row.id != recovered.id)
        .collect::<Vec<_>>();
    assert_eq!(sorted(&unchanged_before), sorted(&unchanged_after));
}

async fn exercise_catalog_append(repository: &ai_gateway::persistence::ControlPlaneRepository) {
    use ai_gateway::persistence::{
        ChannelBatchUpdateInput, ChannelBatchUpdateTarget, RepositoryError,
    };

    let initial = repository.topology().await.unwrap();
    let caps: Vec<_> = initial
        .channel_capabilities
        .iter()
        .filter(|cap| cap.deleted_at.is_none())
        .take(2)
        .collect();
    assert_eq!(caps.len(), 2);
    let targets: Vec<_> = caps
        .iter()
        .map(|cap| ChannelBatchUpdateTarget {
            id: cap.id,
            updated_at: cap.updated_at,
        })
        .collect();
    let changes =
        |name: &str| serde_json::from_value(serde_json::json!({"append_model": name})).unwrap();
    for invalid in [" \t", &"a".repeat(301)] {
        let batch = ChannelBatchUpdateInput {
            items: targets.clone(),
            changes: changes(invalid),
        };
        assert!(matches!(
            repository.prepare_channels_batch(USER, batch).await,
            Err(RepositoryError::Validation)
        ));
    }
    let name = "wire-new";
    let batch = |items| ChannelBatchUpdateInput {
        items,
        changes: changes(name),
    };
    let (first, _) = repository
        .prepare_channels_batch(USER, batch(vec![targets[1].clone()]))
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let first = &first[0];
    assert!(matches!(
        repository
            .prepare_channels_batch(USER, batch(targets.clone()))
            .await,
        Err(RepositoryError::Conflict)
    ));
    let after_conflict = repository.topology().await.unwrap();
    let get = |id| {
        after_conflict
            .channel_capabilities
            .iter()
            .find(|row| row.id == id)
            .unwrap()
    };
    assert_eq!(get(targets[0].id).revision, caps[0].revision);
    assert_eq!(
        get(targets[0].id).settings.available_models,
        caps[0].settings.available_models
    );

    let mut current = targets;
    current[1].updated_at = first.updated_at;
    let (updated, _) = repository
        .prepare_channels_batch(USER, batch(current.clone()))
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].id, current[0].id);
    current[0].updated_at = updated[0].updated_at;
    let appended = repository.topology().await.unwrap();
    for (i, cap) in caps.iter().enumerate() {
        let row = appended
            .channel_capabilities
            .iter()
            .find(|row| row.id == cap.id)
            .unwrap();
        let mut expected = cap.settings.available_models.clone();
        expected.push(name.into());
        assert_eq!(row.settings.available_models, expected);
        assert_eq!(row.updated_at, current[i].updated_at);
    }
    assert!(
        repository
            .prepare_channels_batch(USER, batch(current))
            .await
            .unwrap()
            .commit()
            .await
            .unwrap()
            .0
            .is_empty()
    );
    assert_topology(&appended, &repository.topology().await.unwrap());
}

async fn exercise_model_lifecycle(repository: &ai_gateway::persistence::ControlPlaneRepository) {
    use ai_gateway::persistence::{ControlPlaneMutation, ModelRuleCreateInput};
    let profiles = repository.routing_profiles().await.unwrap();
    assert!(
        profiles
            .iter()
            .any(|profile| profile.id == PROFILE && profile.model_id == MODEL)
    );
    let created = repository.prepare_mutation(USER, ControlPlaneMutation::CreateModel(
        serde_json::from_value(serde_json::json!({
            "source_model_id": "new-profile-model", "display_name": "New profile model",
            "enabled": true, "price_unit_tokens": 1000000, "input_unit_price": "0",
            "cached_input_unit_price": "0", "cache_write_unit_price": "0", "output_unit_price": "0",
            "price_effective_at": chrono::Utc::now()
        })).unwrap()
    )).await.unwrap().commit().await.unwrap().0.remove(0);
    let before = repository.topology().await.unwrap();
    let profile = repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::CreateRule(ModelRuleCreateInput {
                model_id: created.id,
            }),
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    assert!(
        repository
            .routing_profiles()
            .await
            .unwrap()
            .iter()
            .any(|row| row.id == profile.id && row.model_id == created.id)
    );
    assert!(
        repository
            .prepare_mutation(
                USER,
                ControlPlaneMutation::CreateRule(ModelRuleCreateInput {
                    model_id: created.id
                })
            )
            .await
            .is_err()
    );
    let after = repository.topology().await.unwrap();
    assert_eq!(
        sorted(&before.operation_rules),
        sorted(&after.operation_rules)
    );
    assert_eq!(
        sorted(&before.api_key_grants),
        sorted(&after.api_key_grants)
    );

    let model = repository
        .admin_models()
        .await
        .unwrap()
        .into_iter()
        .find(|model| model.id == MODEL)
        .unwrap();
    repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::DeleteModel {
                id: MODEL,
                deleted_by: USER,
                expected_updated_at: model.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let after = repository.topology().await.unwrap();
    assert!(
        after
            .operation_rules
            .iter()
            .filter(|rule| rule.model_routing_profile_id == PROFILE)
            .all(|rule| !rule.enabled)
    );
    assert!(
        after
            .channel_capabilities
            .iter()
            .all(|capability| capability.settings.test_pricing_model_id != Some(MODEL))
    );
    assert!(
        !repository
            .routing_profiles()
            .await
            .unwrap()
            .iter()
            .any(|row| row.model_id == MODEL)
    );
}

async fn exercise_deleted_model_releases_capability(
    repository: ai_gateway::persistence::ControlPlaneRepository,
) {
    let topology = repository.topology().await.unwrap();
    let capability = topology
        .channel_capabilities
        .iter()
        .find(|capability| {
            capability.channel_id == CHANNEL
                && capability.settings.operation
                    == ai_gateway::domain::ApiOperation::ChatCompletions
        })
        .unwrap();
    assert!(
        !topology
            .operation_candidates
            .iter()
            .any(|candidate| candidate.capability_id == capability.id)
    );
    repository
        .prepare_mutation(
            USER,
            ai_gateway::persistence::ControlPlaneMutation::DeleteChannelCapability {
                id: capability.id,
                expected: capability.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(
        repository
            .topology()
            .await
            .unwrap()
            .channel_capabilities
            .iter()
            .find(|row| row.id == capability.id)
            .unwrap()
            .deleted_at
            .is_some()
    );
}

async fn exercise_self_service_channel_boundary(
    repository: &ai_gateway::persistence::ControlPlaneRepository,
    dormant_images: Uuid,
) {
    use ai_gateway::domain::{ApiOperation, CapabilitySettings, RequestCompression};
    use ai_gateway::persistence::{
        ApiKeyCreate, ApiKeyPolicyInput, ChannelCapabilityInput, ControlPlaneMutation,
        LogicalChannelInput, RoutingGroupInput, SelfApiKeyUpdate,
    };
    let group = Uuid::new_v4();
    let channel = Uuid::new_v4();
    let capability = Uuid::new_v4();
    let access = repository
        .topology()
        .await
        .unwrap()
        .logical_channels
        .into_iter()
        .find(|c| c.id == CHANNEL)
        .unwrap()
        .access_id;
    for mutation in [
        ControlPlaneMutation::SaveRoutingGroup {
            id: group,
            expected: None,
            input: RoutingGroupInput {
                name: "Explicit Images scope".into(),
                enabled: true,
            },
        },
        ControlPlaneMutation::SaveLogicalChannel {
            id: channel,
            expected: None,
            input: LogicalChannelInput {
                group_id: group,
                access_id: access,
                credential_id: None,
                name: "Images scope".into(),
                enabled: true,
            },
        },
        ControlPlaneMutation::SaveChannelCapability {
            id: capability,
            expected: None,
            input: ChannelCapabilityInput {
                channel_id: channel,
                settings: CapabilitySettings {
                    operation: ApiOperation::ImagesGeneration,
                    enabled: true,
                    available_models: vec!["image-model".into()],
                    request_compression: RequestCompression::Default,
                    test_model: None,
                    test_pricing_model_id: None,
                    auto_disable_allowed: false,
                },
                status_statistics_enabled: false,
                config_template_id: None,
                override_document: serde_json::json!({}),
                billing_multiplier: rust_decimal::Decimal::ONE,
            },
        },
    ] {
        repository
            .prepare_mutation(USER, mutation)
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();
    }
    let policy = repository
        .admin_api_key_policies()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == POLICY)
        .unwrap();
    repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::UpdateApiKeyPolicy {
                id: POLICY,
                expected_updated_at: policy.updated_at,
                input: ApiKeyPolicyInput {
                    name: "Two selected groups".into(),
                    enabled: true,
                    allowed_group_ids: vec![GROUP, group],
                    allowed_channel_ids: vec![],
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let key = repository
        .prepare_mutation(
            USER,
            ControlPlaneMutation::CreateApiKey(ApiKeyCreate {
                user_id: USER,
                name: "Channel-authorized admin key".into(),
                expires_at: None,
                permissions: vec!["proxy".into()],
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            }),
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    let runtime = repository.load().await.unwrap();
    assert!(
        runtime
            .api_keys
            .iter()
            .find(|record| record.id == key.id)
            .unwrap()
            .allowed_channel_ids
            .contains(&dormant_images)
    );
    let expanded = repository
        .prepare_own_api_key_update(
            USER,
            key.id,
            SelfApiKeyUpdate {
                name: "Added explicit Images scope".into(),
                status: "active".into(),
                expires_at: None,
                allowed_group_ids: vec![GROUP, group],
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
            key.updated_at,
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
    let records = repository.load().await.unwrap();
    let targets = &records
        .api_keys
        .iter()
        .find(|k| k.id == key.id)
        .unwrap()
        .allowed_channel_ids;
    assert!(targets.contains(&dormant_images));
    assert!(targets.contains(&capability));
    repository
        .prepare_own_api_key_update(
            USER,
            key.id,
            SelfApiKeyUpdate {
                name: "Removed explicit Images scope".into(),
                status: "active".into(),
                expires_at: None,
                allowed_group_ids: vec![GROUP],
                allowed_channel_ids: vec![],
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
            expanded.updated_at,
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let records = repository.load().await.unwrap();
    let targets = &records
        .api_keys
        .iter()
        .find(|k| k.id == key.id)
        .unwrap()
        .allowed_channel_ids;
    assert!(targets.contains(&dormant_images));
    assert!(!targets.contains(&capability));
}

async fn exercise_topology_updates(repository: &ControlPlaneRepository) {
    use ai_gateway::persistence::{ControlPlaneMutation as M, RepositoryError};

    let before = repository.topology().await.unwrap();
    let rule = before
        .operation_rules
        .iter()
        .find(|rule| rule.id == RULE)
        .unwrap();
    let mut input = ai_gateway::persistence::upstream_topology::rules::rule_input(&before, rule);
    assert!(!before.api_key_grants.is_empty());
    assert!(!before.policy_grants.is_empty());
    input.routing_tiers[0].candidates[0].weight = 11;
    let update = || M::SaveOperationRule {
        id: RULE,
        input: input.clone(),
        expected_updated_at: Some(rule.updated_at),
    };
    repository
        .prepare_mutation(USER, update())
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        repository.prepare_mutation(USER, update()).await,
        Err(RepositoryError::Conflict)
    ));
    let after = repository.topology().await.unwrap();
    assert_eq!(
        sorted(&before.channel_capabilities),
        sorted(&after.channel_capabilities)
    );
    assert_eq!(
        sorted(&before.api_key_grants),
        sorted(&after.api_key_grants)
    );
    assert_eq!(sorted(&before.policy_grants), sorted(&after.policy_grants));
    assert_eq!(after.operation_candidates[0].weight, 11);

    let logical = after
        .logical_channels
        .iter()
        .find(|channel| channel.id == CHANNEL)
        .unwrap();
    let access = after
        .upstream_accesses
        .iter()
        .find(|access| access.id == logical.access_id)
        .unwrap();
    let mut input = access_input(access);
    input.name = "Renamed access".into();
    let update = || M::UpdateUpstreamAccess {
        id: access.id,
        input: input.clone(),
        expected_updated_at: access.updated_at,
    };
    let mutation = repository
        .prepare_mutation(USER, update())
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0
        .remove(0);
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
        repository.prepare_mutation(USER, update()).await,
        Err(RepositoryError::Conflict)
    ));

    let credential = repository.prepare_mutation(USER, M::CreateUpstreamCredential(
        serde_json::from_value(serde_json::json!({"name":"Scope test","kind":"bearer","secret":"synthetic-test-secret",
                                                   "allowed_base_urls":[access.base_url],"enabled":true})).unwrap(),
    )).await.unwrap().commit().await.unwrap().0.remove(0);
    repository
        .prepare_mutation(
            USER,
            M::SaveLogicalChannel {
                id: CHANNEL,
                expected: Some(logical.updated_at),
                input: ai_gateway::persistence::LogicalChannelInput {
                    group_id: GROUP,
                    access_id: access.id,
                    credential_id: Some(credential.id),
                    name: logical.name.clone(),
                    enabled: false,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    input.base_url = "https://outside-scope.test".into();
    assert!(
        repository
            .prepare_mutation(
                USER,
                M::UpdateUpstreamAccess {
                    id: access.id,
                    input,
                    expected_updated_at: mutation.updated_at
                }
            )
            .await
            .is_err()
    );
}

async fn exercise_codex_credential_lifecycle(repository: &ControlPlaneRepository) {
    use ai_gateway::persistence::CodexCredentialCreate;
    let topology_before = repository.topology().await.unwrap();
    let input = CodexCredentialCreate {
        label: "Imported account".into(),
        enabled: true,
        proxy_id: None,
        quota_threshold_percent: 95,
        base_url: "https://codex.test".into(),
        email: Some("new@test.invalid".into()),
        account_id: Some("new-account".into()),
        user_id: Some("new-user".into()),
        plan_type: None,
        is_fedramp: false,
        id_token: "synthetic-id".into(),
        access_token: "synthetic-access".into(),
        refresh_token: "synthetic-refresh".into(),
        access_token_expires_at: None,
        available_models: vec!["wire".into()],
        quota: None,
    };
    let created = repository
        .prepare_codex_credential_create(USER, input, None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0[0]
        .id;
    assert_eq!(repository.codex_credentials().await.unwrap().len(), 1);
    let row = repository.codex_credential(created).await.unwrap().unwrap();
    assert_eq!(row.available_models, ["wire"]);
    let change = repository
        .prepare_codex_credential_update(
            USER,
            created,
            ai_gateway::persistence::CodexCredentialUpdateInput {
                label: "Renamed account".into(),
                enabled: true,
                proxy_id: None,
                quota_threshold_percent: 90,
            },
            row.updated_at,
        )
        .await
        .unwrap();
    let (mutations, _) = change.commit().await.unwrap();
    assert_eq!(mutations[0].after_redacted["label"], "Renamed account");
    for absent in [
        "base_url",
        "capabilities",
        "access_id",
        "access_revision",
        "access_token",
        "refresh_token",
    ] {
        assert!(mutations[0].after_redacted.get(absent).is_none());
    }
    assert_topology(&topology_before, &repository.topology().await.unwrap());
    let row = repository.codex_credential(created).await.unwrap().unwrap();
    repository
        .prepare_codex_credential_delete(USER, created, row.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(
        repository
            .codex_credential(created)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn postgres_updates_check_versions_and_credential_scope() {
    let database = TestDatabase::new().await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    exercise_topology_updates(&repository).await;
    exercise_codex_credential_lifecycle(&repository).await;
    database.cleanup().await;
}

#[tokio::test]
async fn postgres_group_membership_and_model_lifecycle() {
    let database = TestDatabase::new().await;
    exercise_authorization(ai_gateway::persistence::ControlPlaneRepository::new(
        database.pool.clone(),
    ))
    .await;
    for (statement, target) in [
        (
            "UPDATE channel_capabilities SET test_model='wire-model',test_pricing_model_id=$1 WHERE channel_id=$2 AND operation='chat_completion' AND deleted_at IS NULL",
            CHANNEL,
        ),
        (
            "UPDATE model_operation_rules SET enabled=true WHERE model_routing_profile_id=$2 AND EXISTS(SELECT 1 FROM models WHERE id=$1)",
            PROFILE,
        ),
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(statement))
            .bind(MODEL)
            .bind(target)
            .execute(&database.pool)
            .await
            .unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(sqlx::error::DatabaseError::code)
                .as_deref(),
            Some("23514")
        );
    }
    exercise_deleted_model_releases_capability(
        ai_gateway::persistence::ControlPlaneRepository::new(database.pool.clone()),
    )
    .await;
    let retained: Uuid = sqlx::query_scalar(
        "SELECT id FROM model_rule_identity_registry WHERE canonical_rule_id=$1",
    )
    .bind(RULE)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(retained, RULE);
    database.cleanup().await;
}

#[tokio::test]
async fn postgres_reports_resolve_dispatch_capabilities_to_logical_channels() {
    use ai_gateway::persistence::{CostStatisticsFilter, RequestLogQueries, StatisticsGranularity};
    let database = TestDatabase::new().await;
    let other_group = Uuid::new_v4();
    let other_channel = Uuid::new_v4();
    let other_capability = Uuid::new_v4();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO routing_groups(id,name) VALUES ('{other_group}','Secondary group');
         INSERT INTO upstream_channels(id,group_id,access_id,name) VALUES ('{other_channel}','{other_group}','{ACCESS}','Secondary channel');
         INSERT INTO channel_capabilities(id,channel_id,operation) VALUES ('{other_capability}','{other_channel}','responses');
         INSERT INTO group_identity_registry(id,label,canonical_group_id) VALUES ('{other_group}','Secondary group','{other_group}');
         INSERT INTO channel_identity_registry(id,label,canonical_channel_id,capability_id)
         VALUES ('{other_capability}','Secondary channel','{other_channel}','{other_capability}');"
    ))).execute(&database.pool).await.unwrap();
    let log_id = Uuid::new_v4();
    for (request_id, channel, group, format, operation) in [
        (
            log_id,
            CAPABILITY,
            GROUP,
            "open_ai_chat_completions",
            "chat_completion",
        ),
        (
            Uuid::new_v4(),
            other_capability,
            other_group,
            "open_ai_responses",
            "responses",
        ),
    ] {
        sqlx::query(
            "INSERT INTO request_metering_facts(id,started_at,completed_at,user_id,api_key_id,request_source,
                api_format,api_operation,request_protocol,client_model,outcome,channel_group_id,channel_id,cost_amount,peak_pricing)
             VALUES ($1,now(),now(),$2,$3,'client',$4::api_format,$5,'non_stream','history','failed',$6,$7,0,false)"
        ).bind(request_id).bind(USER).bind(KEY).bind(format).bind(operation).bind(group).bind(channel)
            .execute(&database.pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO request_logs(id,started_at,completed_at,user_id,api_key_id,
             api_format,api_operation,client_model,outcome,channel_group_id,channel_id,cost_amount)
         SELECT id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,
                client_model,outcome,channel_group_id,channel_id,cost_amount FROM request_metering_facts"
    ).execute(&database.pool).await.unwrap();
    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO request_metering_facts(id,started_at,completed_at,user_id,api_key_id,request_source,
                api_format,api_operation,request_protocol,client_model,outcome,channel_group_id,channel_id,cost_amount,peak_pricing)
         VALUES ($1,now(),now(),$2,$3,'client','open_ai_responses','responses','non_stream','history','failed',$4,$5,0,false)"
    ).bind(Uuid::new_v4()).bind(USER).bind(KEY).bind(other_group).bind(other_capability)
        .execute(&mut *transaction).await.unwrap();
    sqlx::query("UPDATE channel_capabilities SET status_statistics_enabled=true")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO channel_capabilities(id,channel_id,operation,available_models,status_statistics_enabled)
         VALUES ($1,$2,'images_generation',ARRAY['idle-image'],true)",
    )
    .bind(Uuid::new_v4())
    .bind(CHANNEL)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let queries = RequestLogQueries::new(database.pool.clone());
    let log = queries.get(log_id).await.unwrap().unwrap();
    assert_eq!(log.channel_name.as_deref(), Some("Primary channel"));
    assert!(
        queries
            .get_for_user(Uuid::new_v4(), log_id)
            .await
            .unwrap()
            .is_none()
    );
    let now = chrono::Utc::now();
    let filter = CostStatisticsFilter {
        started_at: now - chrono::Duration::days(1),
        ended_at: now + chrono::Duration::seconds(1),
        granularity: StatisticsGranularity::Day,
        user_id: None,
        api_key_id: None,
        channel_id: None,
        include_channel_details: true,
    };
    let report = queries
        .metering()
        .cost_statistics(filter.clone())
        .await
        .unwrap();
    assert_eq!(report.summary.request_count, 3);
    assert!(
        report
            .channels
            .iter()
            .any(|channel| channel.id == CHANNEL && channel.name == "Primary channel")
    );
    let secondary = queries
        .metering()
        .cost_statistics(CostStatisticsFilter {
            channel_id: Some(other_channel),
            ..filter
        })
        .await
        .unwrap();
    assert_eq!(secondary.summary.request_count, 2);
    assert_eq!(secondary.channels.len(), 1);
    assert_eq!(secondary.channels[0].id, other_channel);
    assert_eq!(secondary.channels[0].request_count, 2);
    let status = queries
        .channel_group_status(ai_gateway::persistence::ChannelGroupStatusWindow::Last24Hours)
        .await
        .unwrap();
    assert_eq!(
        status
            .models
            .iter()
            .map(|row| row.request_count)
            .sum::<i64>(),
        2
    );
    let group_formats = status
        .groups
        .iter()
        .filter(|group| group.id == GROUP)
        .collect::<Vec<_>>();
    assert_eq!(group_formats.len(), 2);
    let images = group_formats
        .iter()
        .find(|group| group.api_format == "open_ai_images")
        .unwrap();
    assert_eq!(images.models[0].model, "idle-image");
    assert_eq!(images.models[0].request_count, 0);
    sqlx::query(
        "UPDATE channel_capabilities SET status_statistics_enabled=false WHERE channel_id=$1",
    )
    .bind(CHANNEL)
    .execute(&database.pool)
    .await
    .unwrap();
    let status = queries
        .channel_group_status(ai_gateway::persistence::ChannelGroupStatusWindow::Last24Hours)
        .await
        .unwrap();
    assert!(!status.groups.iter().any(|group| group.id == GROUP));
    assert_eq!(
        status
            .models
            .iter()
            .map(|row| row.request_count)
            .sum::<i64>(),
        1
    );
    database.cleanup().await;
}

#[cfg(feature = "sqlite-backend")]
mod sqlite_backend {
    use ai_gateway::persistence::sqlite::SqliteDatabase;
    use std::{fs::Permissions, os::unix::fs::PermissionsExt, sync::Arc};

    use super::*;

    async fn database() -> (tempfile::TempDir, SqliteDatabase) {
        let directory = tempfile::Builder::new()
            .permissions(Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let database = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .unwrap();
        database.install_schema().await.unwrap();
        let mut tx = database.begin_write().await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(seed_sql(true)))
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (directory, database)
    }

    #[tokio::test]
    async fn sqlite_updates_check_versions_and_credential_scope() {
        let (_directory, database) = database().await;
        let database = Arc::new(database);
        let repository = ControlPlaneRepository::from_sqlite(database.clone());
        exercise_topology_updates(&repository).await;
        exercise_codex_credential_lifecycle(&repository).await;
        database.close().await;
    }

    #[tokio::test]
    async fn sqlite_group_membership_and_model_lifecycle() {
        let (_directory, database) = database().await;
        let database = std::sync::Arc::new(database);
        exercise_authorization(
            ai_gateway::persistence::ControlPlaneRepository::from_sqlite(database.clone()),
        )
        .await;
        for (statement, target) in [
            (
                "UPDATE channel_capabilities SET updated_at=ag_now(),test_model='wire-model',test_pricing_model_id=?1 WHERE channel_id=?2 AND operation='chat_completion' AND deleted_at IS NULL",
                CHANNEL,
            ),
            (
                "UPDATE model_operation_rules SET enabled=true WHERE model_routing_profile_id=?2 AND EXISTS(SELECT 1 FROM models WHERE id=?1)",
                PROFILE,
            ),
        ] {
            let mut transaction = database.begin_write().await.unwrap();
            let error = sqlx::query(sqlx::AssertSqlSafe(statement))
                .bind(MODEL.to_string())
                .bind(target.to_string())
                .execute(&mut *transaction)
                .await
                .unwrap_err();
            assert!(
                error
                    .as_database_error()
                    .unwrap()
                    .message()
                    .contains("active_")
            );
            transaction.rollback().await.unwrap();
        }
        exercise_deleted_model_releases_capability(
            ai_gateway::persistence::ControlPlaneRepository::from_sqlite(database.clone()),
        )
        .await;
        let mut reader = database.acquire_read().await.unwrap();
        let retained: String = sqlx::query_scalar(
            "SELECT id FROM model_rule_identity_registry WHERE canonical_rule_id=?",
        )
        .bind(RULE.to_string())
        .fetch_one(&mut *reader)
        .await
        .unwrap();
        assert_eq!(retained, RULE.to_string());
        drop(reader);
        database.close().await;
    }
}
