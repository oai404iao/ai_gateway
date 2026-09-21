//! Database-backed execution of the joint capability cutover.
//!
//! Each backend loads the frozen legacy configuration from its own pre-cutover
//! tables, runs the backend-independent [`super::transfer::transfer`] planner,
//! and inserts every canonical row in foreign-key order. Authentication
//! material is never selected: static secrets and OAuth tokens stay in the
//! credential tables.
//!
//! The destination tables must be empty. Inserts are plain (no `ON CONFLICT`),
//! so a partially populated destination or a duplicate id fails the statement;
//! the caller owns the transaction and therefore the rollback.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::transfer::{
    CapabilityCutoverInput, CapabilityCutoverTransfer, CapabilityCutoverTransferError,
};
use crate::domain::{ApiOperation, CapabilityTransport};
use crate::persistence::upstream_topology::GrantOriginKind;
use crate::persistence::{
    ModelRoutingProfileBinding, ModelRuleRecord, ModelRuleRoutingTier, RepositoryError,
};

#[cfg(feature = "sqlite-backend")]
use sqlx::SqliteConnection;

/// Fail-closed cutover execution errors. They never carry authentication
/// material, URLs, wire model values, or secret text.
#[derive(Debug, thiserror::Error)]
pub enum CapabilityCutoverIoError {
    #[error("capability-cutover destination tables are not empty")]
    DestinationNotEmpty,
    #[error("persisted capability-cutover input row is invalid")]
    InvalidRow,
    #[error(transparent)]
    Transfer(#[from] CapabilityCutoverTransferError),
    #[error("capability-cutover persistence operation failed")]
    Storage(#[source] RepositoryError),
}

impl From<sqlx::Error> for CapabilityCutoverIoError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(RepositoryError::from(error))
    }
}

impl From<RepositoryError> for CapabilityCutoverIoError {
    fn from(error: RepositoryError) -> Self {
        Self::Storage(error)
    }
}

/// One frozen model-protocol rule row plus its pricing and profile context.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenModelRule {
    id: Uuid,
    client_model: String,
    api_format: String,
    #[serde(default)]
    routing_tiers: Vec<ModelRuleRoutingTier>,
    model_id: Uuid,
    model_enabled: bool,
    model_currency: String,
    price_unit_tokens: i64,
    price_effective_at: DateTime<Utc>,
    input_unit_price: Decimal,
    cached_input_unit_price: Decimal,
    cache_write_unit_price: Decimal,
    output_unit_price: Decimal,
    advanced_billing: Value,
    enabled: bool,
}

impl FrozenModelRule {
    fn into_record(self) -> ModelRuleRecord {
        ModelRuleRecord {
            id: self.id,
            client_model: self.client_model,
            api_operation: ApiOperation::for_legacy_format(&self.api_format),
            api_format: self.api_format,
            model_id: self.model_id,
            model_enabled: self.model_enabled,
            model_currency: self.model_currency,
            price_unit_tokens: self.price_unit_tokens,
            price_effective_at: self.price_effective_at,
            input_unit_price: self.input_unit_price,
            cached_input_unit_price: self.cached_input_unit_price,
            cache_write_unit_price: self.cache_write_unit_price,
            output_unit_price: self.output_unit_price,
            advanced_billing: self.advanced_billing,
            routing_tiers: self.routing_tiers,
            enabled: self.enabled,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenProfile {
    id: Uuid,
    model_id: Uuid,
    model_enabled: bool,
}

impl From<FrozenProfile> for ModelRoutingProfileBinding {
    fn from(profile: FrozenProfile) -> Self {
        Self {
            id: profile.id,
            model_id: profile.model_id,
            model_enabled: profile.model_enabled,
            model_deleted: false,
        }
    }
}

fn decode_rows<T: for<'de> Deserialize<'de>>(
    rows: Vec<String>,
) -> Result<Vec<T>, CapabilityCutoverIoError> {
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).map_err(|_| CapabilityCutoverIoError::InvalidRow))
        .collect()
}

fn transport_name(transport: CapabilityTransport) -> &'static str {
    match transport {
        CapabilityTransport::HttpJson => "http_json",
        CapabilityTransport::HttpSse => "http_sse",
        CapabilityTransport::Websocket => "websocket",
        CapabilityTransport::Multipart => "multipart",
    }
}

fn origin_kind_name(kind: GrantOriginKind) -> &'static str {
    match kind {
        GrantOriginKind::Group => "group",
        GrantOriginKind::Channel => "channel",
        GrantOriginKind::Capability => "capability",
    }
}

/// Loads all legacy configuration and writes the canonical topology.
///
/// `cutover_at` timestamps every row the legacy schema does not timestamp
/// (operation rules, tiers, grants) so a retried migration stays deterministic.
pub async fn pg_transfer(
    connection: &mut PgConnection,
    cutover_at: DateTime<Utc>,
) -> Result<CapabilityCutoverTransfer, CapabilityCutoverIoError> {
    if pg_destination_has_rows(connection).await? {
        return Err(CapabilityCutoverIoError::DestinationNotEmpty);
    }
    let input = CapabilityCutoverInput {
        groups: decode_rows(
            sqlx::query_scalar::<_, String>(PG_GROUPS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        channels: decode_rows(
            sqlx::query_scalar::<_, String>(PG_CHANNELS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        projections: decode_rows(
            sqlx::query_scalar::<_, String>(PG_PROJECTIONS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        codex_credentials: decode_rows(
            sqlx::query_scalar::<_, String>(
                "SELECT jsonb_build_object('id',channel_id,'enabled',enabled,'deleted_at',deleted_at)::text \
                 FROM codex_oauth_credentials ORDER BY channel_id",
            ).fetch_all(&mut *connection).await?,
        )?,
        keys: decode_rows(
            sqlx::query_scalar::<_, String>(PG_KEYS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        policies: decode_rows(
            sqlx::query_scalar::<_, String>(PG_POLICIES)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        rule_identities: decode_rows(
            sqlx::query_scalar::<_, String>(
                "SELECT jsonb_build_object('id',r.id,'label',m.source_model_id,\
                 'created_at',r.created_at)::text FROM model_rules r \
                 JOIN model_routing_profiles p ON p.id=r.model_routing_profile_id \
                 JOIN models m ON m.id=p.model_id ORDER BY r.id",
            )
            .fetch_all(&mut *connection)
            .await?,
        )?,
        cutover_at,
    };
    let rules = decode_rows::<FrozenModelRule>(
        sqlx::query_scalar::<_, String>(PG_MODEL_RULES)
            .fetch_all(&mut *connection)
            .await?,
    )?
    .into_iter()
    .map(FrozenModelRule::into_record)
    .collect::<Vec<_>>();
    let profiles = decode_rows::<FrozenProfile>(
        sqlx::query_scalar::<_, String>(PG_PROFILES)
            .fetch_all(&mut *connection)
            .await?,
    )?
    .into_iter()
    .map(ModelRoutingProfileBinding::from)
    .collect::<Vec<_>>();

    let output = super::transfer::transfer(&input, &rules, &profiles)?;
    pg_insert(&mut *connection, &output).await?;
    Ok(output)
}

async fn pg_destination_has_rows(
    connection: &mut PgConnection,
) -> Result<bool, CapabilityCutoverIoError> {
    Ok(sqlx::query_scalar::<_, bool>(DESTINATION_HAS_ROWS)
        .fetch_one(&mut *connection)
        .await?)
}

async fn pg_insert(
    connection: &mut PgConnection,
    output: &CapabilityCutoverTransfer,
) -> Result<(), CapabilityCutoverIoError> {
    for record in &output.topology.routing_groups {
        sqlx::query(PG_INSERT_GROUPS)
            .bind(record.id)
            .bind(&record.name)
            .bind(record.enabled)
            .bind(record.sharing_only)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.deleted_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.upstream_accesses {
        sqlx::query(PG_INSERT_ACCESSES)
            .bind(record.id)
            .bind(&record.name)
            .bind(record.connector_kind.as_str())
            .bind(&record.base_url)
            .bind(record.proxy_id)
            .bind(record.connect_timeout_ms)
            .bind(record.response_header_timeout_ms)
            .bind(record.stream_idle_timeout_ms)
            .bind(record.enabled)
            .bind(record.revision)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.deleted_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.logical_channels {
        sqlx::query(PG_INSERT_CHANNELS)
            .bind(record.id)
            .bind(record.group_id)
            .bind(record.access_id)
            .bind(record.credential_id)
            .bind(&record.name)
            .bind(record.enabled)
            .bind(record.binding_revision)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.deleted_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.channel_capabilities {
        let transports = record
            .settings
            .transports
            .iter()
            .map(|transport| transport_name(*transport).to_owned())
            .collect::<Vec<_>>();
        sqlx::query(PG_INSERT_CAPABILITIES)
            .bind(record.id)
            .bind(record.channel_id)
            .bind(record.settings.operation.as_str())
            .bind(&transports)
            .bind(record.settings.enabled)
            .bind(&record.settings.available_models)
            .bind(record.settings.request_compression.as_str())
            .bind(&record.settings.test_model)
            .bind(record.settings.test_pricing_model_id)
            .bind(record.auto_disabled)
            .bind(&record.auto_disable_reason)
            .bind(record.auto_disable_at)
            .bind(record.settings.auto_disable_allowed)
            .bind(record.status_statistics_enabled)
            .bind(record.config_template_id)
            .bind(&record.override_document)
            .bind(record.billing_multiplier)
            .bind(record.revision)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.deleted_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.operation_rules {
        sqlx::query(PG_INSERT_OPERATION_RULES)
            .bind(record.id)
            .bind(record.model_routing_profile_id)
            .bind(record.operation.as_str())
            .bind(record.enabled)
            .bind(record.created_at)
            .bind(record.updated_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.operation_tiers {
        sqlx::query(PG_INSERT_TIERS)
            .bind(record.id)
            .bind(record.rule_id)
            .bind(record.operation.as_str())
            .bind(record.priority)
            .bind(&record.strategy)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.operation_candidates {
        sqlx::query(PG_INSERT_CANDIDATES)
            .bind(record.tier_id)
            .bind(record.operation.as_str())
            .bind(record.capability_id)
            .bind(&record.upstream_model)
            .bind(record.weight)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.api_key_grants {
        sqlx::query(PG_INSERT_KEY_GRANTS)
            .bind(record.api_key_id)
            .bind(record.capability_id)
            .bind(origin_kind_name(record.origin_kind))
            .bind(record.origin_id)
            .bind(record.created_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.policy_grants {
        sqlx::query(PG_INSERT_POLICY_GRANTS)
            .bind(record.policy_id)
            .bind(record.capability_id)
            .bind(origin_kind_name(record.origin_kind))
            .bind(record.origin_id)
            .bind(record.created_at)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.group_identity_registry {
        sqlx::query(PG_INSERT_GROUP_HISTORY)
            .bind(record.id)
            .bind(&record.label)
            .bind(record.created_at)
            .bind(record.canonical_group_id)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.channel_identity_registry {
        sqlx::query(PG_INSERT_CHANNEL_HISTORY)
            .bind(record.id)
            .bind(&record.label)
            .bind(record.created_at)
            .bind(record.canonical_channel_id)
            .bind(record.codex_credential_id)
            .bind(record.capability_id)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.rule_identity_registry {
        sqlx::query(
            "INSERT INTO model_rule_identity_registry \
             (id,label,created_at,canonical_rule_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(record.id)
        .bind(&record.label)
        .bind(record.created_at)
        .bind(record.canonical_rule_id)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

const DESTINATION_HAS_ROWS: &str = r"
SELECT EXISTS (SELECT 1 FROM routing_groups)
    OR EXISTS (SELECT 1 FROM upstream_accesses)
    OR EXISTS (SELECT 1 FROM upstream_channels)
    OR EXISTS (SELECT 1 FROM channel_capabilities)
    OR EXISTS (SELECT 1 FROM model_operation_rules)
    OR EXISTS (SELECT 1 FROM model_capability_tiers)
    OR EXISTS (SELECT 1 FROM model_capability_candidates)
    OR EXISTS (SELECT 1 FROM api_key_capability_grants)
    OR EXISTS (SELECT 1 FROM api_key_policy_capability_grants)
    OR EXISTS (SELECT 1 FROM group_identity_registry)
    OR EXISTS (SELECT 1 FROM channel_identity_registry)
    OR EXISTS (SELECT 1 FROM model_rule_identity_registry)";

const PG_GROUPS: &str = r"
SELECT jsonb_build_object(
    'id', g.id,
    'name', g.name,
    'api_format', g.api_format::text,
    'connector_kind', g.connector_kind,
    'connector_pool_id', g.connector_pool_id,
    'enabled', g.enabled,
    'sharing_only', g.sharing_only,
    'request_compression', g.request_compression,
    'status_statistics_enabled', g.status_statistics_enabled,
    'created_at', g.created_at,
    'updated_at', g.updated_at,
    'deleted_at', g.deleted_at
)::text
FROM channel_groups AS g
ORDER BY g.id";

const PG_CHANNELS: &str = r"
SELECT jsonb_build_object(
    'id', c.id,
    'channel_group_id', c.channel_group_id,
    'api_format', c.api_format::text,
    'name', c.name,
    'base_url', c.base_url,
    'enabled', c.enabled,
    'auto_disabled', c.auto_disabled,
    'auto_disabled_reason', c.auto_disabled_reason,
    'auto_disable_allowed', c.auto_disable_allowed,
    'billing_multiplier', c.billing_multiplier::text,
    'proxy_id', c.proxy_id,
    'config_template_id', c.config_template_id,
    'override_document', c.override_document,
    'connect_timeout_ms', c.connect_timeout_ms,
    'response_header_timeout_ms', c.response_header_timeout_ms,
    'stream_idle_timeout_ms', c.stream_idle_timeout_ms,
    'credential_id', c.credential_id,
    'credential_binding_revision', c.credential_binding_revision,
    'supports_websocket', c.supports_websocket,
    'supports_standalone_web_search', c.supports_standalone_web_search,
    'available_models', c.available_models,
    'test_model', c.test_model,
    'test_pricing_model_id', c.test_pricing_model_id,
    'created_at', c.created_at,
    'updated_at', c.updated_at,
    'deleted_at', c.deleted_at
)::text
FROM channels AS c
ORDER BY c.id";

const PG_PROJECTIONS: &str = r"
SELECT jsonb_build_object(
    'credential_id', p.credential_id,
    'api_format', p.api_format::text,
    'channel_id', p.channel_id
)::text
FROM codex_oauth_credential_channels AS p
ORDER BY p.credential_id, p.api_format, p.channel_id";

const PG_KEYS: &str = r"
SELECT jsonb_build_object(
    'key_id', k.id,
    'allowed_api_formats', k.allowed_api_formats::text[],
    'allowed_group_ids', k.allowed_group_ids,
    'allowed_channel_ids', k.allowed_channel_ids
)::text
FROM api_keys AS k
ORDER BY k.id";

const PG_POLICIES: &str = r"
SELECT jsonb_build_object(
    'policy_id', p.id,
    'allowed_group_ids', p.allowed_group_ids,
    'allowed_channel_ids', p.allowed_channel_ids
)::text
FROM api_key_policies AS p
ORDER BY p.id";

const PG_MODEL_RULES: &str = r"
SELECT jsonb_build_object(
    'id', r.id,
    'client_model', m.source_model_id,
    'api_format', r.api_format::text,
    'model_id', m.id,
    'model_enabled', m.enabled,
    'model_currency', m.currency,
    'price_unit_tokens', m.price_unit_tokens,
    'price_effective_at', m.price_effective_at,
    'input_unit_price', m.input_unit_price::text,
    'cached_input_unit_price', m.cached_input_unit_price::text,
    'cache_write_unit_price', m.cache_write_unit_price::text,
    'output_unit_price', m.output_unit_price::text,
    'advanced_billing', m.advanced_billing,
    'routing_tiers', COALESCE((
        SELECT jsonb_agg(jsonb_build_object(
            'priority', tier.priority,
            'selection_strategy', tier.selection_strategy,
            'candidates', COALESCE((
                SELECT jsonb_agg(jsonb_build_object(
                    'channel_id', candidate.channel_id,
                    'upstream_model', candidate.upstream_model,
                    'weight', candidate.weight
                ) ORDER BY candidate.channel_id, candidate.upstream_model)
                FROM model_rule_routing_candidates AS candidate
                WHERE candidate.model_rule_id = tier.model_rule_id
                  AND candidate.priority = tier.priority
            ), '[]'::jsonb)
        ) ORDER BY tier.priority)
        FROM model_rule_routing_tiers AS tier
        WHERE tier.model_rule_id = r.id
    ), '[]'::jsonb),
    'enabled', r.enabled
)::text
FROM model_rules AS r
JOIN model_routing_profiles AS profile ON profile.id = r.model_routing_profile_id
JOIN models AS m ON m.id = profile.model_id AND m.deleted_at IS NULL
ORDER BY r.id";

const PG_PROFILES: &str = r"
SELECT jsonb_build_object(
    'id', profile.id,
    'model_id', profile.model_id,
    'model_enabled', m.enabled
)::text
FROM model_routing_profiles AS profile
JOIN models AS m ON m.id = profile.model_id AND m.deleted_at IS NULL
ORDER BY profile.id";

const PG_INSERT_GROUPS: &str = "INSERT INTO routing_groups \
    (id,name,enabled,sharing_only,created_at,updated_at,deleted_at) \
    VALUES ($1,$2,$3,$4,$5,$6,$7)";

const PG_INSERT_ACCESSES: &str = "INSERT INTO upstream_accesses \
    (id,name,connector_kind,base_url,proxy_id,connect_timeout_ms,response_header_timeout_ms, \
     stream_idle_timeout_ms,enabled,revision,created_at,updated_at,deleted_at) \
    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)";

const PG_INSERT_CHANNELS: &str = "INSERT INTO upstream_channels \
    (id,group_id,access_id,credential_id,name,enabled,binding_revision,created_at,updated_at,deleted_at) \
    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)";

const PG_INSERT_CAPABILITIES: &str = "INSERT INTO channel_capabilities \
    (id,channel_id,operation,transports,enabled,available_models,request_compression,test_model, \
     test_pricing_model_id,auto_disabled,auto_disable_reason,auto_disable_at,auto_disable_allowed, \
     status_statistics_enabled,config_template_id,override_document,billing_multiplier,revision, \
     created_at,updated_at,deleted_at) \
    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)";

const PG_INSERT_OPERATION_RULES: &str = "INSERT INTO model_operation_rules \
    (id,model_routing_profile_id,operation,enabled,created_at,updated_at) \
    VALUES ($1,$2,$3,$4,$5,$6)";

const PG_INSERT_TIERS: &str = "INSERT INTO model_capability_tiers \
    (id,rule_id,operation,priority,strategy) VALUES ($1,$2,$3,$4,$5)";

const PG_INSERT_CANDIDATES: &str = "INSERT INTO model_capability_candidates \
    (tier_id,operation,capability_id,upstream_model,weight) VALUES ($1,$2,$3,$4,$5)";

const PG_INSERT_KEY_GRANTS: &str = "INSERT INTO api_key_capability_grants \
    (api_key_id,capability_id,origin_kind,origin_id,created_at) VALUES ($1,$2,$3,$4,$5)";

const PG_INSERT_POLICY_GRANTS: &str = "INSERT INTO api_key_policy_capability_grants \
    (policy_id,capability_id,origin_kind,origin_id,created_at) VALUES ($1,$2,$3,$4,$5)";

const PG_INSERT_GROUP_HISTORY: &str = "INSERT INTO group_identity_registry \
    (id,label,created_at,canonical_group_id) VALUES ($1,$2,$3,$4)";

const PG_INSERT_CHANNEL_HISTORY: &str = "INSERT INTO channel_identity_registry \
    (id,label,created_at,canonical_channel_id,codex_credential_id,capability_id) \
    VALUES ($1,$2,$3,$4,$5,$6)";

/// Loads all legacy configuration and writes the canonical topology.
///
/// `cutover_at` timestamps every row the legacy schema does not timestamp
/// (operation rules, tiers, grants) so a retried migration stays deterministic.
#[cfg(feature = "sqlite-backend")]
pub async fn sqlite_transfer(
    connection: &mut SqliteConnection,
    cutover_at: DateTime<Utc>,
) -> Result<CapabilityCutoverTransfer, CapabilityCutoverIoError> {
    if sqlite_destination_has_rows(connection).await? {
        return Err(CapabilityCutoverIoError::DestinationNotEmpty);
    }
    let input = CapabilityCutoverInput {
        groups: decode_rows(
            sqlx::query_scalar::<_, String>(SQLITE_GROUPS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        channels: decode_rows(
            sqlx::query_scalar::<_, String>(SQLITE_CHANNELS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        projections: decode_rows(
            sqlx::query_scalar::<_, String>(SQLITE_PROJECTIONS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        codex_credentials: decode_rows(
            sqlx::query_scalar::<_, String>(
                "SELECT json_object('id',channel_id,'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),\
                 'deleted_at',deleted_at) FROM codex_oauth_credentials ORDER BY channel_id",
            ).fetch_all(&mut *connection).await?,
        )?,
        keys: decode_rows(
            sqlx::query_scalar::<_, String>(SQLITE_KEYS)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        policies: decode_rows(
            sqlx::query_scalar::<_, String>(SQLITE_POLICIES)
                .fetch_all(&mut *connection)
                .await?,
        )?,
        rule_identities: decode_rows(
            sqlx::query_scalar::<_, String>(
                "SELECT json_object('id',r.id,'label',m.source_model_id,\
                 'created_at',r.created_at) FROM model_rules r \
                 JOIN model_routing_profiles p ON p.id=r.model_routing_profile_id \
                 JOIN models m ON m.id=p.model_id ORDER BY r.id",
            )
            .fetch_all(&mut *connection)
            .await?,
        )?,
        cutover_at,
    };
    let mut rules = decode_rows::<FrozenModelRule>(
        sqlx::query_scalar::<_, String>(SQLITE_MODEL_RULES)
            .fetch_all(&mut *connection)
            .await?,
    )?
    .into_iter()
    .map(FrozenModelRule::into_record)
    .collect::<Vec<_>>();
    let mut tiers = sqlite_load_tiers(&mut *connection).await?;
    for rule in &mut rules {
        rule.routing_tiers = tiers.remove(&rule.id).unwrap_or_default();
    }
    let profiles = decode_rows::<FrozenProfile>(
        sqlx::query_scalar::<_, String>(SQLITE_PROFILES)
            .fetch_all(&mut *connection)
            .await?,
    )?
    .into_iter()
    .map(ModelRoutingProfileBinding::from)
    .collect::<Vec<_>>();

    let output = super::transfer::transfer(&input, &rules, &profiles)?;
    sqlite_insert(&mut *connection, &output).await?;
    Ok(output)
}

/// Frozen pre-cutover routing tiers, grouped by rule. SQLite stores the flat
/// `(rule, priority, channel, model)` candidate rows introduced by the legacy
/// flat-routing migration.
#[cfg(feature = "sqlite-backend")]
async fn sqlite_load_tiers(
    connection: &mut SqliteConnection,
) -> Result<std::collections::HashMap<Uuid, Vec<ModelRuleRoutingTier>>, CapabilityCutoverIoError> {
    use crate::persistence::ModelRuleRouteCandidate;
    use crate::persistence::sqlite::SqliteUuid;

    let tiers = sqlx::query_as::<_, (SqliteUuid, i32, String)>(
        "SELECT model_rule_id,priority,selection_strategy FROM model_rule_routing_tiers \
         ORDER BY model_rule_id,priority",
    )
    .fetch_all(&mut *connection)
    .await?;
    let candidates = sqlx::query_as::<_, (SqliteUuid, i32, SqliteUuid, String, i32)>(
        "SELECT model_rule_id,priority,channel_id,upstream_model,weight \
         FROM model_rule_routing_candidates \
         ORDER BY model_rule_id,priority,channel_id,upstream_model",
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut grouped = std::collections::HashMap::<Uuid, Vec<ModelRuleRoutingTier>>::new();
    for (rule_id, priority, selection_strategy) in tiers {
        grouped
            .entry(rule_id.0)
            .or_default()
            .push(ModelRuleRoutingTier {
                priority,
                selection_strategy,
                candidates: Vec::new(),
            });
    }
    for (rule_id, priority, channel_id, upstream_model, weight) in candidates {
        let Some(tier) = grouped
            .get_mut(&rule_id.0)
            .and_then(|tiers| tiers.iter_mut().find(|tier| tier.priority == priority))
        else {
            continue;
        };
        tier.candidates.push(ModelRuleRouteCandidate {
            channel_id: channel_id.0,
            upstream_model,
            weight,
        });
    }
    Ok(grouped)
}

#[cfg(feature = "sqlite-backend")]
async fn sqlite_destination_has_rows(
    connection: &mut SqliteConnection,
) -> Result<bool, CapabilityCutoverIoError> {
    Ok(sqlx::query_scalar::<_, bool>(DESTINATION_HAS_ROWS)
        .fetch_one(&mut *connection)
        .await?)
}

#[cfg(feature = "sqlite-backend")]
async fn sqlite_insert(
    connection: &mut SqliteConnection,
    output: &CapabilityCutoverTransfer,
) -> Result<(), CapabilityCutoverIoError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUnitPrice, SqliteUuid};

    let json = |value: &Value| -> Result<String, CapabilityCutoverIoError> {
        serde_json::to_string(value).map_err(|_| CapabilityCutoverIoError::InvalidRow)
    };
    let json_array = |values: &Vec<String>| -> Result<String, CapabilityCutoverIoError> {
        serde_json::to_string(values).map_err(|_| CapabilityCutoverIoError::InvalidRow)
    };
    let multiplier = |value: Decimal| -> Result<SqliteUnitPrice, CapabilityCutoverIoError> {
        SqliteUnitPrice::new(value).map_err(|_| CapabilityCutoverIoError::InvalidRow)
    };

    for record in &output.topology.routing_groups {
        sqlx::query(SQLITE_INSERT_GROUPS)
            .bind(SqliteUuid(record.id))
            .bind(&record.name)
            .bind(record.enabled)
            .bind(record.sharing_only)
            .bind(SqliteTimestamp(record.created_at))
            .bind(SqliteTimestamp(record.updated_at))
            .bind(record.deleted_at.map(SqliteTimestamp))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.upstream_accesses {
        sqlx::query(SQLITE_INSERT_ACCESSES)
            .bind(SqliteUuid(record.id))
            .bind(&record.name)
            .bind(record.connector_kind.as_str())
            .bind(&record.base_url)
            .bind(record.proxy_id.map(SqliteUuid))
            .bind(record.connect_timeout_ms)
            .bind(record.response_header_timeout_ms)
            .bind(record.stream_idle_timeout_ms)
            .bind(record.enabled)
            .bind(SqliteUuid(record.revision))
            .bind(SqliteTimestamp(record.created_at))
            .bind(SqliteTimestamp(record.updated_at))
            .bind(record.deleted_at.map(SqliteTimestamp))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.logical_channels {
        sqlx::query(SQLITE_INSERT_CHANNELS)
            .bind(SqliteUuid(record.id))
            .bind(SqliteUuid(record.group_id))
            .bind(SqliteUuid(record.access_id))
            .bind(record.credential_id.map(SqliteUuid))
            .bind(&record.name)
            .bind(record.enabled)
            .bind(SqliteUuid(record.binding_revision))
            .bind(SqliteTimestamp(record.created_at))
            .bind(SqliteTimestamp(record.updated_at))
            .bind(record.deleted_at.map(SqliteTimestamp))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.channel_capabilities {
        let transports = record
            .settings
            .transports
            .iter()
            .map(|transport| transport_name(*transport).to_owned())
            .collect::<Vec<_>>();
        sqlx::query(SQLITE_INSERT_CAPABILITIES)
            .bind(SqliteUuid(record.id))
            .bind(SqliteUuid(record.channel_id))
            .bind(record.settings.operation.as_str())
            .bind(json_array(&transports)?)
            .bind(record.settings.enabled)
            .bind(json_array(&record.settings.available_models)?)
            .bind(record.settings.request_compression.as_str())
            .bind(&record.settings.test_model)
            .bind(record.settings.test_pricing_model_id.map(SqliteUuid))
            .bind(record.auto_disabled)
            .bind(&record.auto_disable_reason)
            .bind(record.auto_disable_at.map(SqliteTimestamp))
            .bind(record.settings.auto_disable_allowed)
            .bind(record.status_statistics_enabled)
            .bind(record.config_template_id.map(SqliteUuid))
            .bind(json(&record.override_document)?)
            .bind(multiplier(record.billing_multiplier)?)
            .bind(SqliteUuid(record.revision))
            .bind(SqliteTimestamp(record.created_at))
            .bind(SqliteTimestamp(record.updated_at))
            .bind(record.deleted_at.map(SqliteTimestamp))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.operation_rules {
        sqlx::query(SQLITE_INSERT_OPERATION_RULES)
            .bind(SqliteUuid(record.id))
            .bind(SqliteUuid(record.model_routing_profile_id))
            .bind(record.operation.as_str())
            .bind(record.enabled)
            .bind(SqliteTimestamp(record.created_at))
            .bind(SqliteTimestamp(record.updated_at))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.operation_tiers {
        sqlx::query(SQLITE_INSERT_TIERS)
            .bind(SqliteUuid(record.id))
            .bind(SqliteUuid(record.rule_id))
            .bind(record.operation.as_str())
            .bind(record.priority)
            .bind(&record.strategy)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.operation_candidates {
        sqlx::query(SQLITE_INSERT_CANDIDATES)
            .bind(SqliteUuid(record.tier_id))
            .bind(record.operation.as_str())
            .bind(SqliteUuid(record.capability_id))
            .bind(&record.upstream_model)
            .bind(record.weight)
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.api_key_grants {
        sqlx::query(SQLITE_INSERT_KEY_GRANTS)
            .bind(SqliteUuid(record.api_key_id))
            .bind(SqliteUuid(record.capability_id))
            .bind(origin_kind_name(record.origin_kind))
            .bind(SqliteUuid(record.origin_id))
            .bind(SqliteTimestamp(record.created_at))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.topology.policy_grants {
        sqlx::query(SQLITE_INSERT_POLICY_GRANTS)
            .bind(SqliteUuid(record.policy_id))
            .bind(SqliteUuid(record.capability_id))
            .bind(origin_kind_name(record.origin_kind))
            .bind(SqliteUuid(record.origin_id))
            .bind(SqliteTimestamp(record.created_at))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.group_identity_registry {
        sqlx::query(SQLITE_INSERT_GROUP_HISTORY)
            .bind(SqliteUuid(record.id))
            .bind(&record.label)
            .bind(SqliteTimestamp(record.created_at))
            .bind(record.canonical_group_id.map(SqliteUuid))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.channel_identity_registry {
        sqlx::query(SQLITE_INSERT_CHANNEL_HISTORY)
            .bind(SqliteUuid(record.id))
            .bind(&record.label)
            .bind(SqliteTimestamp(record.created_at))
            .bind(record.canonical_channel_id.map(SqliteUuid))
            .bind(record.codex_credential_id.map(SqliteUuid))
            .bind(record.capability_id.map(SqliteUuid))
            .execute(&mut *connection)
            .await?;
    }
    for record in &output.rule_identity_registry {
        sqlx::query(
            "INSERT INTO model_rule_identity_registry \
             (id,label,created_at,canonical_rule_id) VALUES (?,?,?,?)",
        )
        .bind(SqliteUuid(record.id))
        .bind(&record.label)
        .bind(SqliteTimestamp(record.created_at))
        .bind(record.canonical_rule_id.map(SqliteUuid))
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
const SQLITE_GROUPS: &str = r"
SELECT json_object(
    'id', g.id,
    'name', g.name,
    'api_format', g.api_format,
    'connector_kind', g.connector_kind,
    'connector_pool_id', g.connector_pool_id,
    'enabled', json(CASE g.enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'sharing_only', json(CASE g.sharing_only WHEN 1 THEN 'true' ELSE 'false' END),
    'request_compression', g.request_compression,
    'status_statistics_enabled', json(CASE g.status_statistics_enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'created_at', g.created_at,
    'updated_at', g.updated_at,
    'deleted_at', g.deleted_at
)
FROM channel_groups AS g
ORDER BY g.id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_CHANNELS: &str = r"
SELECT json_object(
    'id', c.id,
    'channel_group_id', c.channel_group_id,
    'api_format', c.api_format,
    'name', c.name,
    'base_url', c.base_url,
    'enabled', json(CASE c.enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'auto_disabled', json(CASE c.auto_disabled WHEN 1 THEN 'true' ELSE 'false' END),
    'auto_disabled_reason', c.auto_disabled_reason,
    'auto_disable_allowed', json(CASE c.auto_disable_allowed WHEN 1 THEN 'true' ELSE 'false' END),
    'billing_multiplier', c.billing_multiplier,
    'proxy_id', c.proxy_id,
    'config_template_id', c.config_template_id,
    'override_document', json(c.override_document),
    'connect_timeout_ms', c.connect_timeout_ms,
    'response_header_timeout_ms', c.response_header_timeout_ms,
    'stream_idle_timeout_ms', c.stream_idle_timeout_ms,
    'credential_id', c.credential_id,
    'credential_binding_revision', c.credential_binding_revision,
    'supports_websocket', json(CASE c.supports_websocket WHEN 1 THEN 'true' ELSE 'false' END),
    'supports_standalone_web_search', json(CASE c.supports_standalone_web_search WHEN 1 THEN 'true' ELSE 'false' END),
    'available_models', json(c.available_models),
    'test_model', c.test_model,
    'test_pricing_model_id', c.test_pricing_model_id,
    'created_at', c.created_at,
    'updated_at', c.updated_at,
    'deleted_at', c.deleted_at
)
FROM channels AS c
ORDER BY c.id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_PROJECTIONS: &str = r"
SELECT json_object(
    'credential_id', p.credential_id,
    'api_format', p.api_format,
    'channel_id', p.channel_id
)
FROM codex_oauth_credential_channels AS p
ORDER BY p.credential_id, p.api_format, p.channel_id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_KEYS: &str = r"
SELECT json_object(
    'key_id', k.id,
    'allowed_api_formats', json(k.allowed_api_formats),
    'allowed_group_ids', json(k.allowed_group_ids),
    'allowed_channel_ids', json(k.allowed_channel_ids)
)
FROM api_keys AS k
ORDER BY k.id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_POLICIES: &str = r"
SELECT json_object(
    'policy_id', p.id,
    'allowed_group_ids', json(p.allowed_group_ids),
    'allowed_channel_ids', json(p.allowed_channel_ids)
)
FROM api_key_policies AS p
ORDER BY p.id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_MODEL_RULES: &str = r"
SELECT json_object(
    'id', r.id,
    'client_model', m.source_model_id,
    'api_format', r.api_format,
    'model_id', m.id,
    'model_enabled', json(CASE m.enabled WHEN 1 THEN 'true' ELSE 'false' END),
    'model_currency', m.currency,
    'price_unit_tokens', m.price_unit_tokens,
    'price_effective_at', m.price_effective_at,
    'input_unit_price', m.input_unit_price,
    'cached_input_unit_price', m.cached_input_unit_price,
    'cache_write_unit_price', m.cache_write_unit_price,
    'output_unit_price', m.output_unit_price,
    'advanced_billing', json(m.advanced_billing),
    'enabled', json(CASE r.enabled WHEN 1 THEN 'true' ELSE 'false' END)
)
FROM model_rules AS r
JOIN model_routing_profiles AS profile ON profile.id = r.model_routing_profile_id
JOIN models AS m ON m.id = profile.model_id AND m.deleted_at IS NULL
ORDER BY r.id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_PROFILES: &str = r"
SELECT json_object(
    'id', profile.id,
    'model_id', profile.model_id,
    'model_enabled', json(CASE m.enabled WHEN 1 THEN 'true' ELSE 'false' END)
)
FROM model_routing_profiles AS profile
JOIN models AS m ON m.id = profile.model_id AND m.deleted_at IS NULL
ORDER BY profile.id";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_GROUPS: &str = "INSERT INTO routing_groups \
    (id,name,enabled,sharing_only,created_at,updated_at,deleted_at) \
    VALUES (?1,?2,?3,?4,?5,?6,?7)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_ACCESSES: &str = "INSERT INTO upstream_accesses \
    (id,name,connector_kind,base_url,proxy_id,connect_timeout_ms,response_header_timeout_ms, \
     stream_idle_timeout_ms,enabled,revision,created_at,updated_at,deleted_at) \
    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_CHANNELS: &str = "INSERT INTO upstream_channels \
    (id,group_id,access_id,credential_id,name,enabled,binding_revision,created_at,updated_at,deleted_at) \
    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_CAPABILITIES: &str = "INSERT INTO channel_capabilities \
    (id,channel_id,operation,transports,enabled,available_models,request_compression,test_model, \
     test_pricing_model_id,auto_disabled,auto_disable_reason,auto_disable_at,auto_disable_allowed, \
     status_statistics_enabled,config_template_id,override_document,billing_multiplier,revision, \
     created_at,updated_at,deleted_at) \
    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_OPERATION_RULES: &str = "INSERT INTO model_operation_rules \
    (id,model_routing_profile_id,operation,enabled,created_at,updated_at) \
    VALUES (?1,?2,?3,?4,?5,?6)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_TIERS: &str = "INSERT INTO model_capability_tiers \
    (id,rule_id,operation,priority,strategy) VALUES (?1,?2,?3,?4,?5)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_CANDIDATES: &str = "INSERT INTO model_capability_candidates \
    (tier_id,operation,capability_id,upstream_model,weight) VALUES (?1,?2,?3,?4,?5)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_KEY_GRANTS: &str = "INSERT INTO api_key_capability_grants \
    (api_key_id,capability_id,origin_kind,origin_id,created_at) VALUES (?1,?2,?3,?4,?5)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_POLICY_GRANTS: &str = "INSERT INTO api_key_policy_capability_grants \
    (policy_id,capability_id,origin_kind,origin_id,created_at) VALUES (?1,?2,?3,?4,?5)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_GROUP_HISTORY: &str = "INSERT INTO group_identity_registry \
    (id,label,created_at,canonical_group_id) VALUES (?1,?2,?3,?4)";

#[cfg(feature = "sqlite-backend")]
const SQLITE_INSERT_CHANNEL_HISTORY: &str = "INSERT INTO channel_identity_registry \
    (id,label,created_at,canonical_channel_id,codex_credential_id,capability_id) \
    VALUES (?1,?2,?3,?4,?5,?6)";
