//! Typed canonical records and strict admin inputs for the joint
//! capability-cutover topology, with transaction-scoped loaders for both
//! backends.
//!
//! This is a read model over the not-yet-registered `capability_cutover` DDL.
//! Authentication material stays in its own credential tables: nothing here
//! carries OAuth tokens, refresh state, or static secrets.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

#[cfg(feature = "sqlite-backend")]
use sqlx::SqliteConnection;

use super::RepositoryError;
use crate::domain::{ApiOperation, CapabilitySettings, ConnectorKind};

pub mod rules;
mod runtime;
mod snapshot;
pub use runtime::ModelRoutingProfileBinding;
#[cfg(test)]
pub(crate) use runtime::{BaseControlPlaneRecords, resolve_runtime};
pub use snapshot::pg_load_control_plane;
#[cfg(feature = "sqlite-backend")]
pub use snapshot::sqlite_load_control_plane;

/// Fixed provenance of one capability grant.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantOriginKind {
    Group,
    Channel,
    Capability,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingGroupRecord {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub sharing_only: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamAccessRecord {
    pub id: Uuid,
    pub name: String,
    pub connector_kind: ConnectorKind,
    pub base_url: String,
    pub proxy_id: Option<Uuid>,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub enabled: bool,
    pub revision: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// A logical channel binds exactly one access and at most one credential
/// (`credential_id IS NULL` means unauthenticated).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalChannelRecord {
    pub id: Uuid,
    pub group_id: Uuid,
    pub access_id: Uuid,
    pub credential_id: Option<Uuid>,
    pub name: String,
    pub enabled: bool,
    pub binding_revision: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelCapabilityRecord {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub settings: CapabilitySettings,
    pub auto_disabled: bool,
    pub auto_disable_reason: Option<String>,
    pub auto_disable_at: Option<DateTime<Utc>>,
    pub status_statistics_enabled: bool,
    pub config_template_id: Option<Uuid>,
    pub override_document: Value,
    pub billing_multiplier: Decimal,
    pub revision: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRuleRecord {
    pub id: Uuid,
    pub model_routing_profile_id: Uuid,
    pub operation: ApiOperation,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationTierRecord {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub operation: ApiOperation,
    pub priority: i32,
    pub strategy: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationCandidateRecord {
    pub tier_id: Uuid,
    pub operation: ApiOperation,
    pub capability_id: Uuid,
    pub upstream_model: String,
    pub weight: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyCapabilityGrantRecord {
    pub api_key_id: Uuid,
    pub capability_id: Uuid,
    pub origin_kind: GrantOriginKind,
    pub origin_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyPolicyCapabilityGrantRecord {
    pub policy_id: Uuid,
    pub capability_id: Uuid,
    pub origin_kind: GrantOriginKind,
    pub origin_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// Every canonical row of the joint topology read in one caller transaction.
#[derive(Clone, Debug, Default)]
pub struct UpstreamTopologyRecords {
    pub routing_groups: Vec<RoutingGroupRecord>,
    pub upstream_accesses: Vec<UpstreamAccessRecord>,
    pub logical_channels: Vec<LogicalChannelRecord>,
    pub channel_capabilities: Vec<ChannelCapabilityRecord>,
    pub operation_rules: Vec<OperationRuleRecord>,
    pub operation_tiers: Vec<OperationTierRecord>,
    pub operation_candidates: Vec<OperationCandidateRecord>,
    pub api_key_grants: Vec<ApiKeyCapabilityGrantRecord>,
    pub policy_grants: Vec<ApiKeyPolicyCapabilityGrantRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingGroupInput {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub sharing_only: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamAccessInput {
    pub name: String,
    pub connector_kind: ConnectorKind,
    pub base_url: String,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub connect_timeout_ms: Option<i32>,
    #[serde(default)]
    pub response_header_timeout_ms: Option<i32>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<i32>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalChannelInput {
    pub group_id: Uuid,
    pub access_id: Uuid,
    /// Required and nullable. The field must be present so an update can never
    /// silently detach authentication by omitting it.
    #[serde(deserialize_with = "required_nullable_uuid")]
    pub credential_id: Option<Uuid>,
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelCapabilityInput {
    pub channel_id: Uuid,
    pub settings: CapabilitySettings,
    #[serde(default)]
    pub status_statistics_enabled: bool,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    #[serde(default = "empty_object")]
    pub override_document: Value,
    #[serde(default = "default_billing_multiplier")]
    pub billing_multiplier: Decimal,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRuleInput {
    pub model_routing_profile_id: Uuid,
    pub operation: ApiOperation,
    pub enabled: bool,
    pub routing_tiers: Vec<OperationTierInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationTierInput {
    pub priority: i32,
    pub selection_strategy: String,
    pub candidates: Vec<OperationCandidateInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperationCandidateInput {
    pub capability_id: Uuid,
    pub upstream_model: String,
    pub weight: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyCapabilityGrantInput {
    pub api_key_id: Uuid,
    pub capability_id: Uuid,
    pub origin_kind: GrantOriginKind,
    pub origin_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyPolicyCapabilityGrantInput {
    pub policy_id: Uuid,
    pub capability_id: Uuid,
    pub origin_kind: GrantOriginKind,
    pub origin_id: Uuid,
}

fn required_nullable_uuid<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
    Option::<Uuid>::deserialize(deserializer)
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn default_billing_multiplier() -> Decimal {
    Decimal::ONE
}

fn decode_json_rows<T: for<'de> Deserialize<'de>>(
    rows: Vec<String>,
) -> Result<Vec<T>, RepositoryError> {
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).map_err(|_| RepositoryError::Validation))
        .collect()
}

async fn pg_decode<T: for<'de> Deserialize<'de>>(
    connection: &mut PgConnection,
    sql: &'static str,
) -> Result<Vec<T>, RepositoryError> {
    let rows = sqlx::query_scalar::<_, String>(sql)
        .fetch_all(connection)
        .await?;
    decode_json_rows(rows)
}

#[cfg(feature = "sqlite-backend")]
async fn sqlite_decode<T: for<'de> Deserialize<'de>>(
    connection: &mut SqliteConnection,
    sql: &'static str,
) -> Result<Vec<T>, RepositoryError> {
    let rows = sqlx::query_scalar::<_, String>(sql)
        .fetch_all(connection)
        .await?;
    decode_json_rows(rows)
}

/// Reads every topology row inside the caller's transaction.
///
/// `billing_multiplier` is projected as text because `row_to_json` would round
/// the `numeric(24, 12)` value through a binary float.
pub async fn pg_load(
    connection: &mut PgConnection,
) -> Result<UpstreamTopologyRecords, RepositoryError> {
    Ok(UpstreamTopologyRecords {
        routing_groups: pg_decode(connection, PG_ROUTING_GROUPS).await?,
        upstream_accesses: pg_decode(connection, PG_UPSTREAM_ACCESSES).await?,
        logical_channels: pg_decode(connection, PG_LOGICAL_CHANNELS).await?,
        channel_capabilities: pg_decode(connection, PG_CHANNEL_CAPABILITIES).await?,
        operation_rules: pg_decode(connection, PG_OPERATION_RULES).await?,
        operation_tiers: pg_decode(connection, PG_OPERATION_TIERS).await?,
        operation_candidates: pg_decode(connection, PG_OPERATION_CANDIDATES).await?,
        api_key_grants: pg_decode(connection, PG_API_KEY_GRANTS).await?,
        policy_grants: pg_decode(connection, PG_POLICY_GRANTS).await?,
    })
}

/// Reads every topology row inside the caller's transaction.
///
/// SQLite stores booleans as integers and arrays/documents as JSON text, so
/// they are re-typed with `json(...)` before the row reaches serde.
#[cfg(feature = "sqlite-backend")]
pub async fn sqlite_load(
    connection: &mut SqliteConnection,
) -> Result<UpstreamTopologyRecords, RepositoryError> {
    Ok(UpstreamTopologyRecords {
        routing_groups: sqlite_decode(connection, SQLITE_ROUTING_GROUPS).await?,
        upstream_accesses: sqlite_decode(connection, SQLITE_UPSTREAM_ACCESSES).await?,
        logical_channels: sqlite_decode(connection, SQLITE_LOGICAL_CHANNELS).await?,
        channel_capabilities: sqlite_decode(connection, SQLITE_CHANNEL_CAPABILITIES).await?,
        operation_rules: sqlite_decode(connection, SQLITE_OPERATION_RULES).await?,
        operation_tiers: sqlite_decode(connection, SQLITE_OPERATION_TIERS).await?,
        operation_candidates: sqlite_decode(connection, SQLITE_OPERATION_CANDIDATES).await?,
        api_key_grants: sqlite_decode(connection, SQLITE_API_KEY_GRANTS).await?,
        policy_grants: sqlite_decode(connection, SQLITE_POLICY_GRANTS).await?,
    })
}

const PG_ROUTING_GROUPS: &str = r"
SELECT jsonb_build_object(
    'id', id,
    'name', name,
    'enabled', enabled,
    'sharing_only', sharing_only,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)::text
FROM routing_groups
ORDER BY id";

const PG_UPSTREAM_ACCESSES: &str = r"
SELECT jsonb_build_object(
    'id', id,
    'name', name,
    'connector_kind', connector_kind,
    'base_url', base_url,
    'proxy_id', proxy_id,
    'connect_timeout_ms', connect_timeout_ms,
    'response_header_timeout_ms', response_header_timeout_ms,
    'stream_idle_timeout_ms', stream_idle_timeout_ms,
    'enabled', enabled,
    'revision', revision,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)::text
FROM upstream_accesses
ORDER BY id";

const PG_LOGICAL_CHANNELS: &str = r"
SELECT jsonb_build_object(
    'id', id,
    'group_id', group_id,
    'access_id', access_id,
    'credential_id', credential_id,
    'name', name,
    'enabled', enabled,
    'binding_revision', binding_revision,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)::text
FROM upstream_channels
ORDER BY id";

const PG_CHANNEL_CAPABILITIES: &str = r"
SELECT jsonb_build_object(
    'id', id,
    'channel_id', channel_id,
    'settings', jsonb_build_object(
        'operation', operation,
        'transports', transports,
        'enabled', enabled,
        'available_models', available_models,
        'request_compression', request_compression,
        'test_model', test_model,
        'test_pricing_model_id', test_pricing_model_id,
        'auto_disable_allowed', auto_disable_allowed
    ),
    'auto_disabled', auto_disabled,
    'auto_disable_reason', auto_disable_reason,
    'auto_disable_at', auto_disable_at,
    'status_statistics_enabled', status_statistics_enabled,
    'config_template_id', config_template_id,
    'override_document', override_document,
    'billing_multiplier', billing_multiplier::text,
    'revision', revision,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)::text
FROM channel_capabilities
ORDER BY id";

const PG_OPERATION_RULES: &str = r"
SELECT jsonb_build_object(
    'id', id,
    'model_routing_profile_id', model_routing_profile_id,
    'operation', operation,
    'enabled', enabled,
    'created_at', created_at,
    'updated_at', updated_at
)::text
FROM model_operation_rules
ORDER BY id";

const PG_OPERATION_TIERS: &str = r"
SELECT jsonb_build_object(
    'id', id,
    'rule_id', rule_id,
    'operation', operation,
    'priority', priority,
    'strategy', strategy
)::text
FROM model_capability_tiers
ORDER BY id";

const PG_OPERATION_CANDIDATES: &str = r"
SELECT jsonb_build_object(
    'tier_id', tier_id,
    'operation', operation,
    'capability_id', capability_id,
    'upstream_model', upstream_model,
    'weight', weight
)::text
FROM model_capability_candidates
ORDER BY tier_id, capability_id, upstream_model";

const PG_API_KEY_GRANTS: &str = r"
SELECT jsonb_build_object(
    'api_key_id', api_key_id,
    'capability_id', capability_id,
    'origin_kind', origin_kind,
    'origin_id', origin_id,
    'created_at', created_at
)::text
FROM api_key_capability_grants
ORDER BY api_key_id, capability_id, origin_kind, origin_id";

const PG_POLICY_GRANTS: &str = r"
SELECT jsonb_build_object(
    'policy_id', policy_id,
    'capability_id', capability_id,
    'origin_kind', origin_kind,
    'origin_id', origin_id,
    'created_at', created_at
)::text
FROM api_key_policy_capability_grants
ORDER BY policy_id, capability_id, origin_kind, origin_id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_ROUTING_GROUPS: &str = r"
SELECT json_object(
    'id', id,
    'name', name,
    'enabled', json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'sharing_only', json(CASE sharing_only WHEN 1 THEN 'true' ELSE 'false' END),
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)
FROM routing_groups
ORDER BY id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_UPSTREAM_ACCESSES: &str = r"
SELECT json_object(
    'id', id,
    'name', name,
    'connector_kind', connector_kind,
    'base_url', base_url,
    'proxy_id', proxy_id,
    'connect_timeout_ms', connect_timeout_ms,
    'response_header_timeout_ms', response_header_timeout_ms,
    'stream_idle_timeout_ms', stream_idle_timeout_ms,
    'enabled', json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'revision', revision,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)
FROM upstream_accesses
ORDER BY id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_LOGICAL_CHANNELS: &str = r"
SELECT json_object(
    'id', id,
    'group_id', group_id,
    'access_id', access_id,
    'credential_id', credential_id,
    'name', name,
    'enabled', json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'binding_revision', binding_revision,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)
FROM upstream_channels
ORDER BY id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_CHANNEL_CAPABILITIES: &str = r"
SELECT json_object(
    'id', id,
    'channel_id', channel_id,
    'settings', json_object(
        'operation', operation,
        'transports', json(transports),
        'enabled', json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),
        'available_models', json(available_models),
        'request_compression', request_compression,
        'test_model', test_model,
        'test_pricing_model_id', test_pricing_model_id,
        'auto_disable_allowed', json(CASE auto_disable_allowed WHEN 1 THEN 'true' ELSE 'false' END)
    ),
    'auto_disabled', json(CASE auto_disabled WHEN 1 THEN 'true' ELSE 'false' END),
    'auto_disable_reason', auto_disable_reason,
    'auto_disable_at', auto_disable_at,
    'status_statistics_enabled', json(CASE status_statistics_enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'config_template_id', config_template_id,
    'override_document', json(override_document),
    'billing_multiplier', billing_multiplier,
    'revision', revision,
    'created_at', created_at,
    'updated_at', updated_at,
    'deleted_at', deleted_at
)
FROM channel_capabilities
ORDER BY id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_OPERATION_RULES: &str = r"
SELECT json_object(
    'id', id,
    'model_routing_profile_id', model_routing_profile_id,
    'operation', operation,
    'enabled', json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'created_at', created_at,
    'updated_at', updated_at
)
FROM model_operation_rules
ORDER BY id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_OPERATION_TIERS: &str = r"
SELECT json_object(
    'id', id,
    'rule_id', rule_id,
    'operation', operation,
    'priority', priority,
    'strategy', strategy
)
FROM model_capability_tiers
ORDER BY id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_OPERATION_CANDIDATES: &str = r"
SELECT json_object(
    'tier_id', tier_id,
    'operation', operation,
    'capability_id', capability_id,
    'upstream_model', upstream_model,
    'weight', weight
)
FROM model_capability_candidates
ORDER BY tier_id, capability_id, upstream_model";

#[cfg(feature = "sqlite-backend")]
const SQLITE_API_KEY_GRANTS: &str = r"
SELECT json_object(
    'api_key_id', api_key_id,
    'capability_id', capability_id,
    'origin_kind', origin_kind,
    'origin_id', origin_id,
    'created_at', created_at
)
FROM api_key_capability_grants
ORDER BY api_key_id, capability_id, origin_kind, origin_id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_POLICY_GRANTS: &str = r"
SELECT json_object(
    'policy_id', policy_id,
    'capability_id', capability_id,
    'origin_kind', origin_kind,
    'origin_id', origin_id,
    'created_at', created_at
)
FROM api_key_policy_capability_grants
ORDER BY policy_id, capability_id, origin_kind, origin_id";

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn capability_settings() -> Value {
        json!({
            "operation": "responses",
            "transports": ["http_json", "http_sse"],
            "enabled": true,
            "available_models": ["gpt-test"],
            "request_compression": "zstd",
            "test_model": null,
            "test_pricing_model_id": null,
            "auto_disable_allowed": true
        })
    }

    #[test]
    fn default_topology_is_empty() {
        let records = UpstreamTopologyRecords::default();
        assert!(records.routing_groups.is_empty());
        assert!(records.upstream_accesses.is_empty());
        assert!(records.logical_channels.is_empty());
        assert!(records.channel_capabilities.is_empty());
        assert!(records.operation_rules.is_empty());
        assert!(records.operation_tiers.is_empty());
        assert!(records.operation_candidates.is_empty());
        assert!(records.api_key_grants.is_empty());
        assert!(records.policy_grants.is_empty());
    }

    #[test]
    fn logical_channel_input_requires_explicit_nullable_credential() {
        let group_id = Uuid::new_v4();
        let access_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let missing = json!({
            "group_id": group_id,
            "access_id": access_id,
            "name": "channel",
            "enabled": true
        });
        assert!(serde_json::from_value::<LogicalChannelInput>(missing).is_err());

        let null = json!({
            "group_id": group_id,
            "access_id": access_id,
            "credential_id": null,
            "name": "channel",
            "enabled": true
        });
        let input =
            serde_json::from_value::<LogicalChannelInput>(null).expect("nullable credential");
        assert_eq!(input.credential_id, None);

        let bound = json!({
            "group_id": group_id,
            "access_id": access_id,
            "credential_id": credential_id,
            "name": "channel",
            "enabled": false
        });
        let input = serde_json::from_value::<LogicalChannelInput>(bound).expect("bound credential");
        assert_eq!(input.credential_id, Some(credential_id));
    }

    #[test]
    fn capability_input_reuses_strict_settings() {
        let channel_id = Uuid::new_v4();
        let input: ChannelCapabilityInput = serde_json::from_value(json!({
            "channel_id": channel_id,
            "settings": capability_settings()
        }))
        .expect("settings input");
        assert_eq!(input.settings.operation, ApiOperation::Responses);
        assert!(input.settings.enabled);
        assert_eq!(input.billing_multiplier, Decimal::ONE);
        assert_eq!(input.override_document, json!({}));
        assert!(!input.status_statistics_enabled);

        let mut unknown_top = json!({
            "channel_id": channel_id,
            "settings": capability_settings()
        });
        unknown_top["unexpected"] = json!(true);
        assert!(serde_json::from_value::<ChannelCapabilityInput>(unknown_top).is_err());

        let mut unknown_settings = capability_settings();
        unknown_settings["bogus"] = json!(1);
        assert!(
            serde_json::from_value::<ChannelCapabilityInput>(json!({
                "channel_id": channel_id,
                "settings": unknown_settings
            }))
            .is_err()
        );

        let mut missing_setting = capability_settings();
        missing_setting
            .as_object_mut()
            .expect("object")
            .remove("transports");
        assert!(
            serde_json::from_value::<ChannelCapabilityInput>(json!({
                "channel_id": channel_id,
                "settings": missing_setting
            }))
            .is_err()
        );
    }

    #[test]
    fn inputs_apply_documented_defaults_and_reject_unknown_fields() {
        let group: RoutingGroupInput =
            serde_json::from_value(json!({"name": "group", "enabled": true})).expect("group");
        assert!(!group.sharing_only);

        let access: UpstreamAccessInput = serde_json::from_value(json!({
            "name": "access",
            "connector_kind": "codex_oauth",
            "base_url": "https://upstream.example",
            "enabled": true
        }))
        .expect("access");
        assert_eq!(access.connector_kind, ConnectorKind::CodexOauth);
        assert_eq!(access.proxy_id, None);
        assert_eq!(access.connect_timeout_ms, None);

        assert!(
            serde_json::from_value::<RoutingGroupInput>(json!({
                "name": "group",
                "enabled": true,
                "api_format": "open_ai_responses"
            }))
            .is_err()
        );
    }

    #[test]
    fn canonical_records_decode_loader_json() {
        let id = Uuid::new_v4();
        let channel_id = Uuid::new_v4();
        let revision = Uuid::new_v4();
        let record: ChannelCapabilityRecord = serde_json::from_value(json!({
            "id": id,
            "channel_id": channel_id,
            "settings": capability_settings(),
            "auto_disabled": true,
            "auto_disable_reason": "upstream timeout",
            "auto_disable_at": "2026-09-20T15:44:11.123456+00:00",
            "status_statistics_enabled": true,
            "config_template_id": null,
            "override_document": {"temperature": 0.5},
            "billing_multiplier": "1.250000000000",
            "revision": revision,
            "created_at": "2026-09-20T15:44:11.123456+00:00",
            "updated_at": "2026-09-21T15:44:11.123456+00:00",
            "deleted_at": null
        }))
        .expect("capability record");
        assert_eq!(record.settings.operation, ApiOperation::Responses);
        assert_eq!(record.channel_id, channel_id);
        assert!(record.auto_disabled);
        assert_eq!(
            record.auto_disable_reason.as_deref(),
            Some("upstream timeout")
        );
        assert!(record.auto_disable_at.is_some());
        assert_eq!(
            record.billing_multiplier,
            "1.25".parse::<Decimal>().unwrap()
        );
        assert_eq!(record.revision, revision);

        let channel: LogicalChannelRecord = serde_json::from_value(json!({
            "id": Uuid::new_v4(),
            "group_id": Uuid::new_v4(),
            "access_id": Uuid::new_v4(),
            "credential_id": null,
            "name": "channel",
            "enabled": true,
            "binding_revision": Uuid::new_v4(),
            "created_at": "2026-09-20T15:44:11.123456+00:00",
            "updated_at": "2026-09-20T15:44:11.123456+00:00",
            "deleted_at": "2026-09-22T15:44:11.123456+00:00"
        }))
        .expect("channel record");
        assert_eq!(channel.credential_id, None);
        assert!(channel.deleted_at.is_some());
    }

    #[test]
    fn grant_records_use_typed_origin_kind() {
        let grant: ApiKeyCapabilityGrantRecord = serde_json::from_value(json!({
            "api_key_id": Uuid::new_v4(),
            "capability_id": Uuid::new_v4(),
            "origin_kind": "capability",
            "origin_id": Uuid::new_v4(),
            "created_at": "2026-09-20T15:44:11.123456+00:00"
        }))
        .expect("grant record");
        assert_eq!(grant.origin_kind, GrantOriginKind::Capability);
        assert!(serde_json::from_value::<GrantOriginKind>(json!("unknown")).is_err());

        let policy: ApiKeyPolicyCapabilityGrantInput = serde_json::from_value(json!({
            "policy_id": Uuid::new_v4(),
            "capability_id": Uuid::new_v4(),
            "origin_kind": "group",
            "origin_id": Uuid::new_v4()
        }))
        .expect("policy grant input");
        assert_eq!(policy.origin_kind, GrantOriginKind::Group);
    }

    #[test]
    fn operation_rule_records_carry_operation_scope() {
        let tier: OperationTierRecord = serde_json::from_value(json!({
            "id": Uuid::new_v4(),
            "rule_id": Uuid::new_v4(),
            "operation": "images_edit",
            "priority": 0,
            "strategy": "weighted_random"
        }))
        .expect("tier record");
        assert_eq!(tier.operation, ApiOperation::ImagesEdit);

        let rule: OperationRuleInput = serde_json::from_value(json!({
            "model_routing_profile_id": Uuid::new_v4(),
            "operation": "standalone_web_search",
            "enabled": true,
            "routing_tiers": [{
                "priority": 0,
                "selection_strategy": "weighted_random",
                "candidates": [{
                    "capability_id": Uuid::new_v4(),
                    "upstream_model": "wire-model",
                    "weight": 3
                }]
            }]
        }))
        .expect("atomic rule input");
        assert_eq!(rule.operation, ApiOperation::StandaloneWebSearch);
        assert_eq!(rule.routing_tiers[0].candidates[0].weight, 3);
    }
}
