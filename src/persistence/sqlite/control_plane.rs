//! SQLite persistence for the ordinary control plane: runtime snapshots, Console
//! lists and details, audit reads, self-service API-key options, catalog
//! synchronization, and prepared administrative changes.
//!
//! Writes run on the single shared `BEGIN IMMEDIATE` writer. SQLite cannot
//! re-derive `updated_at` in a trigger the way PostgreSQL's `set_updated_at`
//! does, so every update to a timestamp-guarded table sets `updated_at=ag_now()`
//! explicitly; the connection clock installed by `SqliteDatabase` supplies
//! transaction time. Audit payloads are projected in Rust and mirror the fields
//! PostgreSQL builds in SQL, including decimal amounts as JSON numbers. Codex
//! credential create/update/delete, token refresh/reset, and their paired
//! projections belong to a later slice and are not defined here; Codex sharing
//! policies and the ordinary connector-pool group/credential-independent
//! operations are.
//!
//! Reads run inside a transaction so one snapshot spans every record query.
//! Deletion-impact tokens, prepared-change ordering, and mutation results match
//! the PostgreSQL repository's neutral contracts.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use regex::Regex;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Connection, FromRow, Sqlite, SqliteConnection, Transaction, pool::PoolConnection, types::Json,
};
use uuid::Uuid;

use crate::{
    domain::{ApiFormat, AutomaticDisableTrigger, RequestCompression},
    persistence::{
        ApiKeyPolicyInput, ApiKeyRecord, ApiKeyTargetChannel, ApiKeyTargetGroup,
        ChannelBatchUpdateInput, ChannelDeletionImpact, ChannelDeletionPlan, ChannelDeletionTarget,
        ChannelGroupInput, ChannelGroupRecord, ChannelMutationInput, ChannelRecord,
        ConfigTemplateMutationInput, ConfigTemplateRecord, ConsoleApiKey, ConsoleAuditLog,
        ControlPlaneApiKey, ControlPlaneApiKeyPolicy, ControlPlaneChannel,
        ControlPlaneChannelDetail, ControlPlaneChannelGroup, ControlPlaneConfigTemplate,
        ControlPlaneConfigTemplateDetail, ControlPlaneLists, ControlPlaneModel,
        ControlPlaneModelProtocolRule, ControlPlaneModelProtocolRuleRow, ControlPlaneModelRule,
        ControlPlaneModelRuleRow, ControlPlaneMutation, ControlPlaneProxy, ControlPlaneRecords,
        ControlPlaneUser, ControlPlaneUserGroup, DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID,
        DeletionImpactApiKey, DeletionImpactApiKeyPolicy, DeletionImpactApiKeyPolicyRow,
        DeletionImpactApiKeyRow, DeletionImpactChannel, DeletionImpactChannelRow,
        DeletionImpactFingerprint, DeletionImpactRootRow, DeletionImpactRuleState,
        DeletionImpactUserGroup, DeletionImpactVisibilityRow, FORWARDING_SETTINGS_KEY, ModelInput,
        ModelProtocolRuleCreateInput, ModelProtocolRuleInput, ModelRecord, ModelRuleCreateInput,
        ModelRuleRecord, ModelRuleRoutingStatus, ModelRuleRoutingTier, MutationResult,
        ProxyCreateInput, ProxyInput, ProxyRecord, RepositoryError, RuntimeConfigRecords,
        SYSTEM_PROBE_API_KEY_ID, SYSTEM_PROBE_API_KEY_NAME, SYSTEM_PROBE_DISPLAY_NAME,
        SYSTEM_PROBE_USER_ID, SelfApiKeyChannelOption, SelfApiKeyCreate, SelfApiKeyCurrent,
        SelfApiKeyGroupOption, SelfApiKeyOptions, SelfApiKeyPolicy, SelfApiKeySharingAccess,
        SelfApiKeySharingCredentialOption, SelfApiKeyUpdate, SystemProbeIdentity,
        SystemSettingsInput, SystemSettingsRecord, SystemSettingsView, UserBatchUpdateInput,
        UserGroupInput, UserInput, UserSettingsInput, UserSettingsView, UserUpdateInput,
        deleted_api_key_secret, deletion_impact_for_rule, generate_api_key_secret, sha256_hex,
        system_settings_audit_value, system_settings_view, validate_system_settings_input,
    },
};

use super::{
    SqliteAmount, SqliteDatabase, SqliteOpenError, SqliteSharingAmount, SqliteTimestamp,
    SqliteUnitPrice, SqliteUuid,
};

/// SQLite implementation of the ordinary control-plane repository. The parent
/// backend facade dispatches to this type; it is also directly constructible so
/// backend contract tests exercise the same neutral DTOs.
#[derive(Clone)]
pub struct SqliteControlPlaneRepository {
    pub(super) database: Arc<SqliteDatabase>,
}

impl SqliteControlPlaneRepository {
    pub async fn upstream_credentials(
        &self,
    ) -> Result<Vec<crate::persistence::UpstreamCredentialView>, RepositoryError> {
        let mut connection = self.read().await?;
        let mut tx = connection.begin().await?;
        let records = super::upstream_credentials::records(&mut tx).await?;
        let bindings = super::upstream_credentials::bindings(&mut tx).await?;
        tx.commit().await?;
        Ok(records
            .iter()
            .filter(|record| record.deleted_at.is_none())
            .map(|record| record.view(&bindings))
            .collect())
    }

    pub async fn upstream_credential_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<crate::persistence::UpstreamCredentialDetail>, RepositoryError> {
        let mut connection = self.read().await?;
        let mut tx = connection.begin().await?;
        let record = super::upstream_credentials::records(&mut tx)
            .await?
            .into_iter()
            .find(|record| record.id == id && record.deleted_at.is_none());
        let bindings = super::upstream_credentials::bindings(&mut tx).await?;
        tx.commit().await?;
        Ok(
            record.map(|record| crate::persistence::UpstreamCredentialDetail {
                credential: record.view(&bindings),
                secret: record.secret,
            }),
        )
    }

    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    async fn read(&self) -> Result<PoolConnection<Sqlite>, RepositoryError> {
        self.database.acquire_read().await.map_err(open_failure)
    }

    async fn write(&self) -> Result<Transaction<'static, Sqlite>, RepositoryError> {
        self.database.begin_write().await.map_err(open_failure)
    }

    /// Reads control-plane records from one read snapshot.
    pub async fn load(&self) -> Result<ControlPlaneRecords, RepositoryError> {
        let mut connection = self.read().await?;
        let mut transaction = connection.begin().await?;
        let records = Self::load_transaction(&mut transaction).await?;
        transaction.commit().await?;
        Ok(records)
    }

    /// Loads every record needed to build one coherent data-plane snapshot,
    /// including Codex sharing groups and sharing-only channel protections.
    pub async fn load_runtime(&self) -> Result<RuntimeConfigRecords, RepositoryError> {
        let mut connection = self.read().await?;
        let mut transaction = connection.begin().await?;
        let records = Self::load_runtime_transaction(&mut transaction).await?;
        transaction.commit().await?;
        Ok(records)
    }

    async fn load_runtime_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<RuntimeConfigRecords, RepositoryError> {
        let control_plane = Self::load_transaction(transaction).await?;
        let system_settings = Self::load_system_settings_transaction(transaction).await?;
        let sharing = load_sharing_transaction(&mut *transaction).await?;
        let sharing_only_channels = load_sharing_only_channels(&mut *transaction).await?;
        Ok(RuntimeConfigRecords {
            control_plane,
            system_settings,
            sharing,
            sharing_only_channels,
        })
    }

    async fn load_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<ControlPlaneRecords, RepositoryError> {
        let connection = &mut **transaction;
        let api_keys = sqlx::query_as::<_, ApiKeyRecordRow>(
            "SELECT k.id,k.user_id,u.status AS user_status,u.websocket_enabled AS user_websocket_enabled, \
                    g.filter_fast_mode AS user_filter_fast_mode,k.secret_value,k.status,k.expires_at, \
                    k.allowed_api_formats,k.permissions,k.allowed_group_ids,k.allowed_channel_ids, \
                    k.requests_per_minute,k.max_concurrent_requests,k.quota_limit_amount,k.quota_used_amount \
             FROM api_keys AS k \
             JOIN users AS u ON u.id=k.user_id AND u.deleted_at IS NULL \
             JOIN user_groups AS g ON g.id=u.user_group_id AND g.deleted_at IS NULL \
             WHERE k.is_system=0 AND k.deleted_at IS NULL ORDER BY k.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ApiKeyRecordRow::into_record)
        .collect::<Result<Vec<_>, _>>()?;
        let models = sqlx::query_as::<_, ModelRecordRow>(
            "SELECT id,source_model_id,currency,price_unit_tokens,price_effective_at,input_unit_price, \
                    cached_input_unit_price,cache_write_unit_price,output_unit_price,advanced_billing \
             FROM models WHERE deleted_at IS NULL ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ModelRecordRow::into_record)
        .collect::<Result<Vec<_>, _>>()?;
        let rule_rows = sqlx::query_as::<_, RuntimeRuleRow>(
            "SELECT r.id,m.source_model_id AS client_model,r.api_format AS api_format,m.id AS model_id, \
                    m.enabled AS model_enabled,m.currency AS model_currency,m.price_unit_tokens, \
                    m.price_effective_at,m.input_unit_price,m.cached_input_unit_price, \
                    m.cache_write_unit_price,m.output_unit_price,m.advanced_billing,r.enabled \
             FROM model_rules AS r \
             JOIN model_routing_profiles AS profile ON profile.id=r.model_routing_profile_id \
             JOIN models AS m ON m.id=profile.model_id AND m.deleted_at IS NULL \
             ORDER BY r.id",
        )
        .fetch_all(&mut *connection)
        .await?;
        let mut tiers_by_rule = load_routing_tiers(connection).await?;
        let mut model_rules = Vec::with_capacity(rule_rows.len());
        for row in rule_rows {
            let routing_tiers = tiers_by_rule.remove(&row.id.0).unwrap_or_default();
            model_rules.push(row.into_record(routing_tiers)?);
        }
        let groups = sqlx::query_as::<_, ChannelGroupRow>(
            "SELECT id,name,api_format,connector_kind,request_compression,sharing_only,enabled \
             FROM channel_groups WHERE deleted_at IS NULL ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ChannelGroupRow::into_record)
        .collect::<Vec<_>>();
        let mut channels = sqlx::query_as::<_, ChannelRecordRow>(
            "SELECT c.id,c.channel_group_id,c.api_format,c.name,c.base_url,c.enabled, \
                    c.supports_websocket,c.supports_standalone_web_search,c.auto_disabled, \
                    c.auto_disable_allowed,c.billing_multiplier,c.proxy_id,c.config_template_id, \
                    c.override_document,c.connect_timeout_ms,c.response_header_timeout_ms, \
                    c.stream_idle_timeout_ms,c.upstream_auth_kind,c.upstream_auth_header_name, \
                    c.upstream_api_key,c.available_models,c.test_model,c.test_pricing_model_id \
             FROM channels AS c \
             JOIN channel_groups AS g ON g.id=c.channel_group_id AND g.deleted_at IS NULL \
             WHERE c.deleted_at IS NULL ORDER BY c.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ChannelRecordRow::into_record)
        .collect::<Result<Vec<_>, _>>()?;
        crate::persistence::upstream_credentials::resolve_bindings(
            &mut channels,
            &groups,
            &super::upstream_credentials::records(connection).await?,
            &super::upstream_credentials::bindings(connection).await?,
        )?;
        let proxies = sqlx::query_as::<_, ProxyRecordRow>(
            "SELECT id,name,proxy_url,username,password,no_proxy_hosts,enabled FROM proxies ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ProxyRecordRow::into_record)
        .collect::<Result<Vec<_>, _>>()?;
        let templates = sqlx::query_as::<_, ConfigTemplateRecordRow>(
            "SELECT id,name,description,document,enabled FROM config_templates ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ConfigTemplateRecordRow::into_record)
        .collect::<Result<Vec<_>, _>>()?;
        Ok(ControlPlaneRecords {
            api_keys,
            models,
            model_rules,
            groups,
            channels,
            proxies,
            templates,
        })
    }

    async fn load_system_settings_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<SystemSettingsRecord, RepositoryError> {
        let row = sqlx::query_as::<_, SystemSettingsRow>(
            "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=?",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        row.into_record()
    }
}

/// The SQLite opener reports closed, fenced, or ownership failures as
/// configuration errors; callers treat them as internal storage failures.
fn open_failure(error: SqliteOpenError) -> RepositoryError {
    RepositoryError::from(sqlx::Error::Configuration(Box::new(error)))
}

fn normalize_numeric(value: Decimal, scale: u32) -> Decimal {
    value.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero)
}

fn amount_24_8(value: Decimal) -> Result<SqliteAmount, RepositoryError> {
    SqliteAmount::new(normalize_numeric(value, 8)).map_err(|_| RepositoryError::Validation)
}

fn amount_20_8(value: Decimal) -> Result<SqliteSharingAmount, RepositoryError> {
    SqliteSharingAmount::new(normalize_numeric(value, 8)).map_err(|_| RepositoryError::Validation)
}

fn amount_24_12(value: Decimal) -> Result<SqliteUnitPrice, RepositoryError> {
    SqliteUnitPrice::new(normalize_numeric(value, 12)).map_err(|_| RepositoryError::Validation)
}

/// PostgreSQL renders `numeric` as a JSON number when an audit object is built
/// in SQL; `rust_decimal` would serialize it as a JSON string instead.
fn json_decimal(value: Decimal) -> Value {
    serde_json::from_str(&value.to_string()).unwrap_or(Value::Null)
}

fn json_option_decimal(value: Option<Decimal>) -> Value {
    value.map_or(Value::Null, json_decimal)
}

/// Mirrors the PostgreSQL audit projection of a proxy URL: drop userinfo and
/// everything from the first query or fragment delimiter.
fn audit_proxy_url(value: &str) -> String {
    let without_userinfo = Regex::new(r"^([^:/?#]+://)[^/?#]*@")
        .expect("static proxy userinfo pattern")
        .replace(value, "$1");
    Regex::new(r"[?#].*$")
        .expect("static proxy delimiter pattern")
        .replace(&without_userinfo, "")
        .into_owned()
}

fn json_text<T: serde::Serialize>(value: &T) -> Result<String, RepositoryError> {
    serde_json::to_string(value).map_err(|_| RepositoryError::Validation)
}

fn value_to_text(value: &Value) -> Result<String, RepositoryError> {
    serde_json::to_string(value).map_err(|_| RepositoryError::Validation)
}

fn parse_json(text: &str) -> Result<Value, RepositoryError> {
    serde_json::from_str(text).map_err(|_| RepositoryError::Validation)
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

/// JSON columns are read as TEXT and parsed explicitly so schema validity is
/// enforced by the database rather than assumed by the driver.
fn json_column(text: &str) -> Result<Value, RepositoryError> {
    parse_json(text)
}

fn json_column_optional(text: Option<String>) -> Result<Option<Value>, RepositoryError> {
    text.map(|value| parse_json(&value)).transpose()
}

#[derive(FromRow)]
struct ApiKeyRecordRow {
    id: SqliteUuid,
    user_id: SqliteUuid,
    user_status: String,
    user_websocket_enabled: bool,
    user_filter_fast_mode: bool,
    secret_value: String,
    status: String,
    expires_at: Option<SqliteTimestamp>,
    allowed_api_formats: String,
    permissions: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<SqliteAmount>,
    quota_used_amount: SqliteAmount,
}

impl ApiKeyRecordRow {
    fn into_record(self) -> Result<ApiKeyRecord, RepositoryError> {
        Ok(ApiKeyRecord {
            id: self.id.0,
            user_id: self.user_id.0,
            user_status: self.user_status,
            user_websocket_enabled: self.user_websocket_enabled,
            user_filter_fast_mode: self.user_filter_fast_mode,
            secret_value: self.secret_value,
            status: self.status,
            expires_at: self.expires_at.map(|value| value.0),
            allowed_api_formats: string_list(&self.allowed_api_formats)?,
            permissions: string_list(&self.permissions)?,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            requests_per_minute: self.requests_per_minute,
            max_concurrent_requests: self.max_concurrent_requests,
            quota_limit_amount: self.quota_limit_amount.map(|value| value.0),
            quota_used_amount: self.quota_used_amount.0,
        })
    }
}

fn string_list(text: &str) -> Result<Vec<String>, RepositoryError> {
    serde_json::from_str(text).map_err(|_| RepositoryError::Validation)
}

fn uuid_list(text: &str) -> Result<Vec<Uuid>, RepositoryError> {
    serde_json::from_str(text).map_err(|_| RepositoryError::Validation)
}

#[derive(FromRow)]
struct ModelRecordRow {
    id: SqliteUuid,
    source_model_id: String,
    currency: String,
    price_unit_tokens: i64,
    price_effective_at: SqliteTimestamp,
    input_unit_price: SqliteUnitPrice,
    cached_input_unit_price: SqliteUnitPrice,
    cache_write_unit_price: SqliteUnitPrice,
    output_unit_price: SqliteUnitPrice,
    advanced_billing: String,
}

impl ModelRecordRow {
    fn into_record(self) -> Result<ModelRecord, RepositoryError> {
        Ok(ModelRecord {
            id: self.id.0,
            source_model_id: self.source_model_id,
            currency: self.currency,
            price_unit_tokens: self.price_unit_tokens,
            price_effective_at: self.price_effective_at.0,
            input_unit_price: self.input_unit_price.0,
            cached_input_unit_price: self.cached_input_unit_price.0,
            cache_write_unit_price: self.cache_write_unit_price.0,
            output_unit_price: self.output_unit_price.0,
            advanced_billing: json_column(&self.advanced_billing)?,
        })
    }
}

#[derive(FromRow)]
struct RuntimeRuleRow {
    id: SqliteUuid,
    client_model: String,
    api_format: String,
    model_id: SqliteUuid,
    model_enabled: bool,
    model_currency: String,
    price_unit_tokens: i64,
    price_effective_at: SqliteTimestamp,
    input_unit_price: SqliteUnitPrice,
    cached_input_unit_price: SqliteUnitPrice,
    cache_write_unit_price: SqliteUnitPrice,
    output_unit_price: SqliteUnitPrice,
    advanced_billing: String,
    enabled: bool,
}

impl RuntimeRuleRow {
    fn into_record(
        self,
        routing_tiers: Vec<ModelRuleRoutingTier>,
    ) -> Result<ModelRuleRecord, RepositoryError> {
        Ok(ModelRuleRecord {
            id: self.id.0,
            client_model: self.client_model,
            api_format: self.api_format,
            model_id: self.model_id.0,
            model_enabled: self.model_enabled,
            model_currency: self.model_currency,
            price_unit_tokens: self.price_unit_tokens,
            price_effective_at: self.price_effective_at.0,
            input_unit_price: self.input_unit_price.0,
            cached_input_unit_price: self.cached_input_unit_price.0,
            cache_write_unit_price: self.cache_write_unit_price.0,
            output_unit_price: self.output_unit_price.0,
            advanced_billing: json_column(&self.advanced_billing)?,
            routing_tiers,
            enabled: self.enabled,
        })
    }
}

#[derive(FromRow)]
struct ChannelGroupRow {
    id: SqliteUuid,
    name: String,
    api_format: String,
    connector_kind: String,
    request_compression: String,
    sharing_only: bool,
    enabled: bool,
}

impl ChannelGroupRow {
    fn into_record(self) -> ChannelGroupRecord {
        ChannelGroupRecord {
            id: self.id.0,
            name: self.name,
            api_format: self.api_format,
            connector_kind: self.connector_kind,
            request_compression: self.request_compression,
            sharing_only: self.sharing_only,
            enabled: self.enabled,
        }
    }
}

#[derive(FromRow)]
struct ChannelRecordRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    api_format: String,
    name: String,
    base_url: String,
    enabled: bool,
    supports_websocket: bool,
    supports_standalone_web_search: bool,
    auto_disabled: bool,
    auto_disable_allowed: bool,
    billing_multiplier: SqliteUnitPrice,
    proxy_id: Option<SqliteUuid>,
    config_template_id: Option<SqliteUuid>,
    override_document: String,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    upstream_auth_kind: String,
    upstream_auth_header_name: Option<String>,
    upstream_api_key: Option<String>,
    available_models: String,
    test_model: Option<String>,
    test_pricing_model_id: Option<SqliteUuid>,
}

impl ChannelRecordRow {
    fn into_record(self) -> Result<ChannelRecord, RepositoryError> {
        Ok(ChannelRecord {
            credential: None,
            credential_binding_revision: Uuid::nil(),
            id: self.id.0,
            channel_group_id: self.channel_group_id.0,
            api_format: self.api_format,
            name: self.name,
            base_url: self.base_url,
            enabled: self.enabled,
            supports_websocket: self.supports_websocket,
            supports_standalone_web_search: self.supports_standalone_web_search,
            auto_disabled: self.auto_disabled,
            auto_disable_allowed: self.auto_disable_allowed,
            billing_multiplier: self.billing_multiplier.0,
            proxy_id: self.proxy_id.map(|value| value.0),
            config_template_id: self.config_template_id.map(|value| value.0),
            override_document: json_column(&self.override_document)?,
            connect_timeout_ms: self.connect_timeout_ms,
            response_header_timeout_ms: self.response_header_timeout_ms,
            stream_idle_timeout_ms: self.stream_idle_timeout_ms,
            upstream_auth_kind: self.upstream_auth_kind,
            upstream_auth_header_name: self.upstream_auth_header_name,
            upstream_api_key: self.upstream_api_key,
            available_models: string_list(&self.available_models)?,
            test_model: self.test_model,
            test_pricing_model_id: self.test_pricing_model_id.map(|value| value.0),
        })
    }
}

#[derive(FromRow)]
struct ProxyRecordRow {
    id: SqliteUuid,
    name: String,
    proxy_url: String,
    username: Option<String>,
    password: Option<String>,
    no_proxy_hosts: String,
    enabled: bool,
}

impl ProxyRecordRow {
    fn into_record(self) -> Result<ProxyRecord, RepositoryError> {
        Ok(ProxyRecord {
            id: self.id.0,
            name: self.name,
            proxy_url: self.proxy_url,
            username: self.username,
            password: self.password,
            no_proxy_hosts: string_list(&self.no_proxy_hosts)?,
            enabled: self.enabled,
        })
    }
}

#[derive(FromRow)]
struct ConfigTemplateRecordRow {
    id: SqliteUuid,
    name: String,
    description: Option<String>,
    document: String,
    enabled: bool,
}

impl ConfigTemplateRecordRow {
    fn into_record(self) -> Result<ConfigTemplateRecord, RepositoryError> {
        Ok(ConfigTemplateRecord {
            id: self.id.0,
            name: self.name,
            description: self.description,
            document: json_column(&self.document)?,
            enabled: self.enabled,
        })
    }
}

#[derive(FromRow)]
struct SystemSettingsRow {
    setting_key: String,
    value: String,
    updated_at: SqliteTimestamp,
}

impl SystemSettingsRow {
    fn into_record(self) -> Result<SystemSettingsRecord, RepositoryError> {
        Ok(SystemSettingsRecord {
            setting_key: self.setting_key,
            value: parse_json(&self.value)?,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct RoutingTierRow {
    model_rule_id: SqliteUuid,
    priority: i32,
    selection_strategy: String,
}

#[derive(FromRow)]
struct RoutingCandidateRow {
    model_rule_id: SqliteUuid,
    priority: i32,
    channel_id: SqliteUuid,
    upstream_model: String,
    weight: i32,
}

/// Loads every routing tier and candidate, grouped by rule. Both tables are
/// keyed by `(model_rule_id, api_format, priority)` in the database; the format
/// is already fixed by the rule row the tiers belong to.
async fn load_routing_tiers(
    connection: &mut SqliteConnection,
) -> Result<HashMap<Uuid, Vec<ModelRuleRoutingTier>>, RepositoryError> {
    let tiers = sqlx::query_as::<_, RoutingTierRow>(
        "SELECT model_rule_id,priority,selection_strategy FROM model_rule_routing_tiers \
         ORDER BY model_rule_id,priority",
    )
    .fetch_all(&mut *connection)
    .await?;
    let candidates = sqlx::query_as::<_, RoutingCandidateRow>(
        "SELECT model_rule_id,priority,channel_id,upstream_model,weight \
         FROM model_rule_routing_candidates \
         ORDER BY model_rule_id,priority,channel_id,upstream_model",
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut tiers_by_rule = HashMap::<Uuid, Vec<ModelRuleRoutingTier>>::new();
    for tier in tiers {
        tiers_by_rule
            .entry(tier.model_rule_id.0)
            .or_default()
            .push(ModelRuleRoutingTier {
                priority: tier.priority,
                selection_strategy: tier.selection_strategy,
                candidates: Vec::new(),
            });
    }
    for candidate in candidates {
        let Some(tiers) = tiers_by_rule.get_mut(&candidate.model_rule_id.0) else {
            continue;
        };
        let Some(tier) = tiers
            .iter_mut()
            .find(|tier| tier.priority == candidate.priority)
        else {
            continue;
        };
        tier.candidates
            .push(crate::persistence::ModelRuleRouteCandidate {
                channel_id: candidate.channel_id.0,
                upstream_model: candidate.upstream_model,
                weight: candidate.weight,
            });
    }
    Ok(tiers_by_rule)
}

/// Loads the ordered routing tiers of exactly one protocol rule; used by the
/// transaction-scoped audit projection.
async fn load_rule_routing_tiers(
    connection: &mut SqliteConnection,
    model_rule_id: Uuid,
) -> Result<Vec<ModelRuleRoutingTier>, RepositoryError> {
    let tiers = sqlx::query_as::<_, RoutingTierRow>(
        "SELECT model_rule_id,priority,selection_strategy FROM model_rule_routing_tiers \
         WHERE model_rule_id=? ORDER BY priority",
    )
    .bind(SqliteUuid(model_rule_id))
    .fetch_all(&mut *connection)
    .await?;
    let candidates = sqlx::query_as::<_, RoutingCandidateRow>(
        "SELECT model_rule_id,priority,channel_id,upstream_model,weight \
         FROM model_rule_routing_candidates \
         WHERE model_rule_id=? ORDER BY priority,channel_id,upstream_model",
    )
    .bind(SqliteUuid(model_rule_id))
    .fetch_all(&mut *connection)
    .await?;
    let mut result = tiers
        .into_iter()
        .map(|tier| ModelRuleRoutingTier {
            priority: tier.priority,
            selection_strategy: tier.selection_strategy,
            candidates: Vec::new(),
        })
        .collect::<Vec<_>>();
    for candidate in candidates {
        let Some(tier) = result
            .iter_mut()
            .find(|tier| tier.priority == candidate.priority)
        else {
            continue;
        };
        tier.candidates
            .push(crate::persistence::ModelRuleRouteCandidate {
                channel_id: candidate.channel_id.0,
                upstream_model: candidate.upstream_model,
                weight: candidate.weight,
            });
    }
    Ok(result)
}

/// Channels protected because their credential belongs to a sharing-only
/// Codex pool, including channels reachable through the same provider identity
/// in another pool. No token or identity material is returned.
async fn load_sharing_only_channels(
    connection: &mut SqliteConnection,
) -> Result<Vec<Uuid>, RepositoryError> {
    let rows = sqlx::query_as::<_, ChannelIdRow>(
        "WITH restricted AS ( \
             SELECT DISTINCT source.channel_id AS channel_id,source.user_id,source.account_id \
             FROM codex_oauth_credentials AS source \
             JOIN codex_oauth_credential_channels AS source_projection \
               ON source_projection.credential_id=source.channel_id \
             JOIN channels AS source_channel ON source_channel.id=source_projection.channel_id \
             JOIN channel_groups AS source_group ON source_group.id=source_channel.channel_group_id \
             WHERE source_group.sharing_only AND source.deleted_at IS NULL) \
         SELECT projection.channel_id FROM restricted \
         JOIN codex_oauth_credential_channels AS projection \
           ON projection.credential_id=restricted.channel_id \
         UNION \
         SELECT projection.channel_id FROM restricted AS source \
         JOIN codex_oauth_credentials AS alias \
           ON source.user_id=alias.user_id \
          AND COALESCE(source.account_id,'')=COALESCE(alias.account_id,'') \
         JOIN codex_oauth_credential_channels AS projection \
           ON projection.credential_id=alias.channel_id \
         WHERE source.user_id IS NOT NULL AND alias.deleted_at IS NULL",
    )
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows.into_iter().map(|row| row.channel_id.0).collect())
}

#[derive(FromRow)]
struct ChannelIdRow {
    channel_id: SqliteUuid,
}

#[derive(FromRow)]
struct SharingGroupRow {
    id: SqliteUuid,
    credential_id: SqliteUuid,
    provider_account_id: String,
    provider_user_id: String,
    name: String,
    enabled: bool,
    seats: String,
    primary_limit_amount: SqliteSharingAmount,
    secondary_limit_amount: SqliteSharingAmount,
    request_reservation_amount: SqliteSharingAmount,
    user_requests_per_minute: i32,
    group_requests_per_minute: i32,
    user_max_concurrent_requests: i32,
    group_max_concurrent_requests: i32,
    updated_at: SqliteTimestamp,
}

#[derive(FromRow)]
struct SharingProjectionRow {
    credential_id: SqliteUuid,
    channel_id: SqliteUuid,
}

#[derive(FromRow)]
struct SharingWindowRow {
    id: SqliteUuid,
    credential_id: SqliteUuid,
    window_kind: String,
    scheduled_reset_at: SqliteTimestamp,
    used_percent: i32,
    checked_at: SqliteTimestamp,
}

/// Loads the sharing groups, their channel projections, protected identity
/// aliases, and every fully observed provider window.
async fn load_sharing_transaction(
    connection: &mut SqliteConnection,
) -> Result<Vec<crate::domain::codex_sharing::SharingRecord>, RepositoryError> {
    let groups = sqlx::query_as::<_, SharingGroupRow>(
        "SELECT id,credential_id,provider_account_id,provider_user_id,name,enabled,seats, \
                primary_limit_amount,secondary_limit_amount,request_reservation_amount, \
                user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests, \
                group_max_concurrent_requests,updated_at \
         FROM codex_sharing_groups ORDER BY id",
    )
    .fetch_all(&mut *connection)
    .await?;
    let projections = sqlx::query_as::<_, SharingProjectionRow>(
        "SELECT p.credential_id,p.channel_id FROM codex_oauth_credential_channels AS p \
         ORDER BY p.credential_id,p.api_format",
    )
    .fetch_all(&mut *connection)
    .await?;
    // Protected identities follow both the canonical credential and any
    // recognizable provider alias, matching the PostgreSQL reader.
    let protected = sqlx::query_as::<_, SharingProjectionRow>(
        "SELECT s.credential_id AS credential_id,p.channel_id AS channel_id \
         FROM codex_sharing_groups AS s \
         JOIN codex_oauth_credentials AS c ON c.channel_id=s.credential_id \
         JOIN codex_oauth_credential_channels AS p ON p.credential_id=c.channel_id \
         UNION \
         SELECT s.credential_id,p.channel_id \
         FROM codex_sharing_groups AS s \
         JOIN codex_oauth_credentials AS c \
           ON COALESCE(c.account_id,'')=s.provider_account_id \
          AND c.user_id=s.provider_user_id \
         JOIN codex_oauth_credential_channels AS p ON p.credential_id=c.channel_id",
    )
    .fetch_all(&mut *connection)
    .await?;
    let windows = sqlx::query_as::<_, SharingWindowRow>(
        "WITH observed AS ( \
             SELECT p.id,p.credential_id,p.window_kind,p.scheduled_reset_at, \
                    p.last_used_percent AS used_percent,c.quota_checked_at AS checked_at, \
                    ((c.primary_window_seconds IS NOT NULL) \
                     + (c.secondary_window_seconds IS NOT NULL)) AS expected_count, \
                    count(*) OVER (PARTITION BY p.credential_id) AS observed_count \
             FROM codex_quota_window_periods AS p \
             JOIN codex_oauth_credentials AS c ON c.channel_id=p.credential_id \
             JOIN codex_sharing_groups AS s ON s.credential_id=p.credential_id \
             WHERE p.ended_at IS NULL AND c.deleted_at IS NULL AND c.enabled \
               AND p.last_observed_at=c.quota_checked_at \
               AND ((c.primary_used_percent IS NULL) \
                    + (c.primary_window_seconds IS NULL) \
                    + (c.primary_reset_at IS NULL)) IN (0,3) \
               AND ((c.secondary_used_percent IS NULL) \
                    + (c.secondary_window_seconds IS NULL) \
                    + (c.secondary_reset_at IS NULL)) IN (0,3) \
               AND ((p.window_kind='primary' AND p.scheduled_reset_at=c.primary_reset_at \
                     AND p.window_seconds=c.primary_window_seconds \
                     AND p.last_used_percent=c.primary_used_percent) \
                 OR (p.window_kind='secondary' AND p.scheduled_reset_at=c.secondary_reset_at \
                     AND p.window_seconds=c.secondary_window_seconds \
                     AND p.last_used_percent=c.secondary_used_percent))) \
         SELECT id,credential_id,window_kind,scheduled_reset_at,used_percent,checked_at \
         FROM observed WHERE observed_count=expected_count",
    )
    .fetch_all(&mut *connection)
    .await?;

    let mut channel_ids = HashMap::<Uuid, Vec<Uuid>>::new();
    for row in projections {
        channel_ids
            .entry(row.credential_id.0)
            .or_default()
            .push(row.channel_id.0);
    }
    let mut protected_channel_ids = HashMap::<Uuid, Vec<Uuid>>::new();
    for row in protected {
        protected_channel_ids
            .entry(row.credential_id.0)
            .or_default()
            .push(row.channel_id.0);
    }
    let mut windows_by_credential =
        HashMap::<Uuid, Vec<crate::domain::codex_sharing::SharingWindow>>::new();
    for window in windows {
        windows_by_credential
            .entry(window.credential_id.0)
            .or_default()
            .push(crate::domain::codex_sharing::SharingWindow {
                id: window.id.0,
                credential_id: window.credential_id.0,
                window_kind: window.window_kind,
                scheduled_reset_at: window.scheduled_reset_at.0,
                checked_at: window.checked_at.0,
                used_percent: window.used_percent,
            });
    }
    groups
        .into_iter()
        .map(|group| {
            let id = group.id.0;
            let credential_id = group.credential_id.0;
            // The provider identity is persisted for the database's own
            // uniqueness guards and is not part of the compiled registry.
            let _ = (&group.provider_account_id, &group.provider_user_id);
            let seats: Vec<Option<Uuid>> =
                serde_json::from_str(&group.seats).map_err(|_| RepositoryError::Validation)?;
            let policy = crate::domain::codex_sharing::SharingGroupInput {
                credential_id: group.credential_id.0,
                name: group.name,
                enabled: group.enabled,
                seats,
                primary_limit_amount: group.primary_limit_amount.0,
                secondary_limit_amount: group.secondary_limit_amount.0,
                request_reservation_amount: group.request_reservation_amount.0,
                user_requests_per_minute: u32::try_from(group.user_requests_per_minute)
                    .map_err(|_| RepositoryError::Validation)?,
                group_requests_per_minute: u32::try_from(group.group_requests_per_minute)
                    .map_err(|_| RepositoryError::Validation)?,
                user_max_concurrent_requests: u32::try_from(group.user_max_concurrent_requests)
                    .map_err(|_| RepositoryError::Validation)?,
                group_max_concurrent_requests: u32::try_from(group.group_max_concurrent_requests)
                    .map_err(|_| RepositoryError::Validation)?,
            };
            Ok(crate::domain::codex_sharing::SharingRecord {
                group: crate::domain::codex_sharing::SharingGroup {
                    id,
                    policy,
                    updated_at: group.updated_at.0,
                },
                channel_ids: channel_ids.remove(&credential_id).unwrap_or_default(),
                protected_channel_ids: protected_channel_ids
                    .remove(&credential_id)
                    .unwrap_or_default(),
                windows: windows_by_credential
                    .remove(&credential_id)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Console reads
// ---------------------------------------------------------------------------

impl SqliteControlPlaneRepository {
    pub async fn system_settings(&self) -> Result<SystemSettingsView, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, SystemSettingsRow>(
            "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=?",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .fetch_optional(&mut *reader)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        system_settings_view(row.into_record()?)
    }

    pub async fn user_settings(
        &self,
        user_id: Uuid,
    ) -> Result<Option<UserSettingsView>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, UserSettingsRow>(
            "SELECT websocket_enabled,updated_at FROM users \
             WHERE id=? AND status='active' AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(user_id))
        .fetch_optional(&mut *reader)
        .await?;
        Ok(row.map(UserSettingsRow::into_view))
    }

    /// Every Console list query runs on one read snapshot so cross-referenced
    /// routing status is computed from a consistent set of channels and groups.
    pub async fn control_plane_lists(&self) -> Result<ControlPlaneLists, RepositoryError> {
        let mut connection = self.read().await?;
        let mut transaction = connection.begin().await?;
        let lists = self.control_plane_lists_transaction(&mut transaction).await;
        transaction.commit().await?;
        lists
    }

    async fn control_plane_lists_transaction(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<ControlPlaneLists, RepositoryError> {
        let connection = &mut **transaction;
        let users = sqlx::query_as::<_, ControlPlaneUserRow>(
            "SELECT u.id,u.email,u.display_name,u.role,u.status, \
                    (u.password_hash IS NULL AND u.email IS NOT NULL \
                     AND u.status IN ('invited','suspended','disabled')) AS can_reissue_invitation, \
                    u.password_change_required,u.temporary_password_expires_at,u.user_group_id, \
                    u.default_api_key_policy_id, \
                    COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
                        AS effective_api_key_policy_id, \
                    u.websocket_enabled,u.balance_amount,u.created_at,u.updated_at \
             FROM users AS u \
             JOIN user_groups AS g ON g.id=u.user_group_id AND g.deleted_at IS NULL \
             WHERE u.is_system=0 AND u.deleted_at IS NULL ORDER BY u.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneUserRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        let user_groups = sqlx::query_as::<_, ControlPlaneUserGroupRow>(
            "SELECT g.id,g.name,g.description,g.default_api_key_policy_id, \
                    COALESCE((SELECT json_group_array(visibility.channel_group_id) \
                              FROM user_group_codex_quota_visibility AS visibility \
                              WHERE visibility.user_group_id=g.id \
                              ORDER BY visibility.channel_group_id),'[]') \
                        AS visible_codex_quota_group_ids, \
                    g.filter_fast_mode,g.system_role, \
                    count(CASE WHEN u.deleted_at IS NULL AND u.is_system=0 THEN u.id END) \
                        AS member_count, \
                    g.created_at,g.updated_at \
             FROM user_groups AS g \
             LEFT JOIN users AS u ON u.user_group_id=g.id \
             WHERE g.deleted_at IS NULL \
             GROUP BY g.id \
             ORDER BY (g.system_role IS NULL),g.system_role,g.name,g.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneUserGroupRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        let models = sqlx::query_as::<_, ControlPlaneModelRow>(
            "SELECT id,source_model_id,display_name,provider_name,enabled,price_unit_tokens, \
                    input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price, \
                    price_effective_at,advanced_billing,last_synced_at,created_at,updated_at \
             FROM models WHERE deleted_at IS NULL ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneModelRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        let api_keys = sqlx::query_as::<_, ControlPlaneApiKeyRow>(
            "SELECT k.id,k.user_id,u.status AS user_status,k.name,k.secret_value AS secret,k.status, \
                    k.expires_at,k.allowed_api_formats,k.permissions,k.allowed_group_ids, \
                    k.allowed_channel_ids,k.requests_per_minute,k.max_concurrent_requests, \
                    k.quota_limit_amount,k.quota_used_amount,k.updated_at \
             FROM api_keys AS k \
             JOIN users AS u ON u.id=k.user_id \
             WHERE k.is_system=0 AND k.deleted_at IS NULL AND u.deleted_at IS NULL ORDER BY k.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneApiKeyRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        let api_key_policies = sqlx::query_as::<_, ControlPlaneApiKeyPolicyRow>(
            "SELECT id,name,allowed_group_ids,allowed_channel_ids,enabled,created_at,updated_at \
             FROM api_key_policies ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneApiKeyPolicyRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        let channel_groups = sqlx::query_as::<_, ControlPlaneChannelGroupRow>(
            "SELECT id,name,api_format,connector_kind,connector_pool_id,request_compression, \
                    sharing_only,enabled,status_statistics_enabled,updated_at \
             FROM channel_groups WHERE deleted_at IS NULL ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneChannelGroupRow::into_view)
        .collect::<Vec<_>>();
        let channels = sqlx::query_as::<_, ConsoleChannelRow>(
            "SELECT c.id,c.channel_group_id,c.api_format,g.connector_kind, \
                    (g.connector_kind <> 'openai_compatible') AS provider_managed, \
                    c.name,c.base_url, \
                    CASE WHEN g.connector_kind='codex_oauth' \
                         THEN (c.enabled AND COALESCE(co.enabled,0)) ELSE c.enabled END AS enabled, \
                    c.supports_websocket,c.supports_standalone_web_search,c.auto_disabled, \
                    c.auto_disabled_reason,c.auto_disable_allowed,c.billing_multiplier,c.proxy_id, \
                    c.config_template_id,c.connect_timeout_ms,c.response_header_timeout_ms, \
                    c.stream_idle_timeout_ms,c.credential_id, \
                    c.available_models,c.test_model,c.test_pricing_model_id,c.created_at,c.updated_at \
             FROM channels AS c \
             JOIN channel_groups AS g ON g.id=c.channel_group_id AND g.deleted_at IS NULL \
             LEFT JOIN codex_oauth_credential_channels AS projection ON projection.channel_id=c.id \
             LEFT JOIN codex_oauth_credentials AS co ON co.channel_id=projection.credential_id \
             WHERE c.deleted_at IS NULL \
               AND (g.connector_kind <> 'codex_oauth' \
                    OR (co.channel_id IS NOT NULL AND co.deleted_at IS NULL)) \
             ORDER BY c.id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ConsoleChannelRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        // The neutral `ControlPlaneModelRule*Row` structs use plain `Uuid`/`DateTime`
        // fields, so they are mapped from TEXT-backed row types rather than
        // decoded directly.
        let model_rule_rows = sqlx::query_as::<_, ModelRuleListViewRow>(
            "SELECT profile.id,model.id AS model_id,model.source_model_id AS client_model, \
                    model.display_name AS model_display_name, \
                    model.provider_name AS model_provider_name,model.enabled AS model_enabled, \
                    profile.created_at,profile.updated_at \
             FROM model_routing_profiles AS profile \
             JOIN models AS model ON model.id=profile.model_id AND model.deleted_at IS NULL \
             ORDER BY model.source_model_id,profile.id",
        )
        .fetch_all(&mut *connection)
        .await?;
        let protocol_rule_rows = sqlx::query_as::<_, ConsoleProtocolRuleRow>(
            "SELECT r.id,r.model_routing_profile_id AS model_rule_id,r.api_format, \
                    m.enabled AS model_enabled,r.description,r.enabled,r.updated_at \
             FROM model_rules AS r \
             JOIN model_routing_profiles AS profile ON profile.id=r.model_routing_profile_id \
             JOIN models AS m ON m.id=profile.model_id AND m.deleted_at IS NULL \
             ORDER BY r.id",
        )
        .fetch_all(&mut *connection)
        .await?;
        let mut tiers_by_rule = load_routing_tiers(connection).await?;
        let mut protocols_by_rule = HashMap::<Uuid, Vec<ControlPlaneModelProtocolRule>>::new();
        for row in protocol_rule_rows {
            // Routing tiers hang off the protocol rule id, while the grouped
            // list is keyed by the routing profile the parent model rule uses.
            let rule_id = row.id.0;
            let routing_tiers = tiers_by_rule.remove(&rule_id).unwrap_or_default();
            protocols_by_rule
                .entry(row.model_rule_id.0)
                .or_default()
                .push(row.into_view(routing_tiers, &channel_groups, &channels));
        }
        let model_rules = model_rule_rows
            .into_iter()
            .map(|row| {
                let protocols = protocols_by_rule.remove(&row.id.0).unwrap_or_default();
                control_plane_model_rule(row.into_row(), protocols)
            })
            .collect();
        let proxies = sqlx::query_as::<_, ControlPlaneProxyRow>(
            "SELECT id,name,proxy_url,no_proxy_hosts,enabled, \
                    (username IS NOT NULL OR password IS NOT NULL) AS credential_configured, \
                    created_at,updated_at \
             FROM proxies ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneProxyRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        let config_templates = sqlx::query_as::<_, ControlPlaneConfigTemplateRow>(
            "SELECT id,name,description, \
                    CASE WHEN json_type(document,'$.api_format')='text' \
                         THEN json_extract(document,'$.api_format') END AS api_format, \
                    enabled,created_at,updated_at \
             FROM config_templates ORDER BY id",
        )
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(ControlPlaneConfigTemplateRow::into_view)
        .collect();
        Ok(ControlPlaneLists {
            users,
            user_groups,
            models,
            api_keys,
            api_key_policies,
            channel_groups,
            channels,
            model_rules,
            proxies,
            config_templates,
        })
    }

    /// Used by proxy diagnostics on behalf of the parent backend facade.
    pub async fn proxy_record(&self, id: Uuid) -> Result<Option<ProxyRecord>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, ProxyRecordRow>(
            "SELECT id,name,proxy_url,username,password,no_proxy_hosts,enabled \
             FROM proxies WHERE id=?",
        )
        .bind(SqliteUuid(id))
        .fetch_optional(&mut *reader)
        .await?;
        row.map(ProxyRecordRow::into_record).transpose()
    }

    pub async fn control_plane_channel_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneChannelDetail>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, ControlPlaneChannelDetailRow>(
            "SELECT c.id,c.channel_group_id,c.api_format,g.connector_kind, \
                    (g.connector_kind <> 'openai_compatible') AS provider_managed, \
                    c.name,c.base_url, \
                    CASE WHEN g.connector_kind='codex_oauth' \
                         THEN (c.enabled AND COALESCE(co.enabled,0)) ELSE c.enabled END AS enabled, \
                    c.supports_websocket,c.supports_standalone_web_search,c.auto_disabled, \
                    c.auto_disabled_reason,c.auto_disable_allowed,c.billing_multiplier,c.proxy_id, \
                    c.config_template_id,c.override_document,c.connect_timeout_ms, \
                    c.response_header_timeout_ms,c.stream_idle_timeout_ms,c.credential_id, \
                    c.available_models,c.test_model,c.test_pricing_model_id,c.created_at,c.updated_at \
             FROM channels AS c \
             JOIN channel_groups AS g ON g.id=c.channel_group_id AND g.deleted_at IS NULL \
             LEFT JOIN codex_oauth_credential_channels AS projection ON projection.channel_id=c.id \
             LEFT JOIN codex_oauth_credentials AS co ON co.channel_id=projection.credential_id \
             WHERE c.id=? AND c.deleted_at IS NULL \
               AND (g.connector_kind <> 'codex_oauth' \
                    OR (co.channel_id IS NOT NULL AND co.deleted_at IS NULL))",
        )
        .bind(SqliteUuid(id))
        .fetch_optional(&mut *reader)
        .await?;
        row.map(ControlPlaneChannelDetailRow::into_view).transpose()
    }

    pub async fn control_plane_config_template_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneConfigTemplateDetail>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, ControlPlaneConfigTemplateDetailRow>(
            "SELECT id,name,description, \
                    CASE WHEN json_type(document,'$.api_format')='text' \
                         THEN json_extract(document,'$.api_format') END AS api_format, \
                    document,enabled,created_at,updated_at \
             FROM config_templates WHERE id=?",
        )
        .bind(SqliteUuid(id))
        .fetch_optional(&mut *reader)
        .await?;
        row.map(ControlPlaneConfigTemplateDetailRow::into_view)
            .transpose()
    }

    pub async fn audit_logs(&self, limit: i64) -> Result<Vec<ConsoleAuditLog>, RepositoryError> {
        let mut reader = self.read().await?;
        let rows = sqlx::query_as::<_, ConsoleAuditLogRow>(
            "SELECT id,occurred_at,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
                    before_redacted,after_redacted,correlation_id,reason \
             FROM audit_logs ORDER BY occurred_at DESC,id DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(&mut *reader)
        .await?;
        rows.into_iter()
            .map(ConsoleAuditLogRow::into_view)
            .collect()
    }

    pub async fn own_api_keys(&self, user_id: Uuid) -> Result<Vec<ConsoleApiKey>, RepositoryError> {
        let mut reader = self.read().await?;
        let rows = sqlx::query_as::<_, ConsoleApiKeyRow>(
            "SELECT id,name,secret_value AS secret,status,expires_at,allowed_api_formats, \
                    permissions,allowed_group_ids,allowed_channel_ids,requests_per_minute, \
                    max_concurrent_requests,quota_limit_amount,quota_used_amount,created_at,updated_at \
             FROM api_keys WHERE user_id=? AND is_system=0 AND deleted_at IS NULL \
             ORDER BY created_at DESC,id DESC",
        )
        .bind(SqliteUuid(user_id))
        .fetch_all(&mut *reader)
        .await?;
        rows.into_iter().map(ConsoleApiKeyRow::into_view).collect()
    }

    pub async fn own_api_key(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleApiKey>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, ConsoleApiKeyRow>(
            "SELECT id,name,secret_value AS secret,status,expires_at,allowed_api_formats, \
                    permissions,allowed_group_ids,allowed_channel_ids,requests_per_minute, \
                    max_concurrent_requests,quota_limit_amount,quota_used_amount,created_at,updated_at \
             FROM api_keys WHERE id=? AND user_id=? AND is_system=0 AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(user_id))
        .fetch_optional(&mut *reader)
        .await?;
        row.map(ConsoleApiKeyRow::into_view).transpose()
    }

    pub async fn model_source_ids(&self) -> Result<Vec<String>, RepositoryError> {
        let mut reader = self.read().await?;
        Ok(sqlx::query_scalar(
            "SELECT source_model_id FROM models WHERE deleted_at IS NULL ORDER BY source_model_id",
        )
        .fetch_all(&mut *reader)
        .await?)
    }
}

#[derive(FromRow)]
struct UserSettingsRow {
    websocket_enabled: bool,
    updated_at: SqliteTimestamp,
}

impl UserSettingsRow {
    fn into_view(self) -> UserSettingsView {
        UserSettingsView {
            websocket_enabled: self.websocket_enabled,
            updated_at: self.updated_at.0,
        }
    }
}

#[derive(FromRow)]
struct ControlPlaneUserRow {
    id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    can_reissue_invitation: bool,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
    user_group_id: SqliteUuid,
    default_api_key_policy_id: Option<SqliteUuid>,
    effective_api_key_policy_id: Option<SqliteUuid>,
    websocket_enabled: bool,
    balance_amount: SqliteAmount,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneUserRow {
    fn into_view(self) -> Result<ControlPlaneUser, RepositoryError> {
        Ok(ControlPlaneUser {
            id: self.id.0,
            email: self.email,
            display_name: self.display_name,
            role: self.role,
            status: self.status,
            can_reissue_invitation: self.can_reissue_invitation,
            password_change_required: self.password_change_required,
            temporary_password_expires_at: self.temporary_password_expires_at.map(|value| value.0),
            user_group_id: self.user_group_id.0,
            default_api_key_policy_id: self.default_api_key_policy_id.map(|value| value.0),
            effective_api_key_policy_id: self.effective_api_key_policy_id.map(|value| value.0),
            websocket_enabled: self.websocket_enabled,
            balance_amount: self.balance_amount.0,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneUserGroupRow {
    id: SqliteUuid,
    name: String,
    description: Option<String>,
    default_api_key_policy_id: Option<SqliteUuid>,
    visible_codex_quota_group_ids: String,
    filter_fast_mode: bool,
    system_role: Option<String>,
    member_count: i64,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneUserGroupRow {
    fn into_view(self) -> Result<ControlPlaneUserGroup, RepositoryError> {
        Ok(ControlPlaneUserGroup {
            id: self.id.0,
            name: self.name,
            description: self.description,
            default_api_key_policy_id: self.default_api_key_policy_id.map(|value| value.0),
            visible_codex_quota_group_ids: uuid_list(&self.visible_codex_quota_group_ids)?,
            filter_fast_mode: self.filter_fast_mode,
            system_role: self.system_role,
            member_count: self.member_count,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneModelRow {
    id: SqliteUuid,
    source_model_id: String,
    display_name: String,
    provider_name: Option<String>,
    enabled: bool,
    price_unit_tokens: i64,
    input_unit_price: SqliteUnitPrice,
    cached_input_unit_price: SqliteUnitPrice,
    cache_write_unit_price: SqliteUnitPrice,
    output_unit_price: SqliteUnitPrice,
    price_effective_at: SqliteTimestamp,
    advanced_billing: String,
    last_synced_at: Option<SqliteTimestamp>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneModelRow {
    fn into_view(self) -> Result<ControlPlaneModel, RepositoryError> {
        Ok(ControlPlaneModel {
            id: self.id.0,
            source_model_id: self.source_model_id,
            display_name: self.display_name,
            provider_name: self.provider_name,
            enabled: self.enabled,
            price_unit_tokens: self.price_unit_tokens,
            input_unit_price: self.input_unit_price.0,
            cached_input_unit_price: self.cached_input_unit_price.0,
            cache_write_unit_price: self.cache_write_unit_price.0,
            output_unit_price: self.output_unit_price.0,
            price_effective_at: self.price_effective_at.0,
            advanced_billing: json_column(&self.advanced_billing)?,
            last_synced_at: self.last_synced_at.map(|value| value.0),
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneApiKeyRow {
    id: SqliteUuid,
    user_id: SqliteUuid,
    user_status: String,
    name: String,
    secret: String,
    status: String,
    expires_at: Option<SqliteTimestamp>,
    allowed_api_formats: String,
    permissions: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<SqliteAmount>,
    quota_used_amount: SqliteAmount,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneApiKeyRow {
    fn into_view(self) -> Result<ControlPlaneApiKey, RepositoryError> {
        Ok(ControlPlaneApiKey {
            id: self.id.0,
            user_id: self.user_id.0,
            user_status: self.user_status,
            name: self.name,
            secret: self.secret,
            status: self.status,
            expires_at: self.expires_at.map(|value| value.0),
            allowed_api_formats: string_list(&self.allowed_api_formats)?,
            permissions: string_list(&self.permissions)?,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            requests_per_minute: self.requests_per_minute,
            max_concurrent_requests: self.max_concurrent_requests,
            quota_limit_amount: self.quota_limit_amount.map(|value| value.0),
            quota_used_amount: self.quota_used_amount.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneApiKeyPolicyRow {
    id: SqliteUuid,
    name: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    enabled: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneApiKeyPolicyRow {
    fn into_view(self) -> Result<ControlPlaneApiKeyPolicy, RepositoryError> {
        Ok(ControlPlaneApiKeyPolicy {
            id: self.id.0,
            name: self.name,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            enabled: self.enabled,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneChannelGroupRow {
    id: SqliteUuid,
    name: String,
    api_format: String,
    connector_kind: String,
    connector_pool_id: Option<SqliteUuid>,
    request_compression: String,
    sharing_only: bool,
    enabled: bool,
    status_statistics_enabled: bool,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneChannelGroupRow {
    fn into_view(self) -> ControlPlaneChannelGroup {
        ControlPlaneChannelGroup {
            id: self.id.0,
            name: self.name,
            api_format: self.api_format,
            connector_kind: self.connector_kind,
            connector_pool_id: self.connector_pool_id.map(|value| value.0),
            request_compression: self.request_compression,
            sharing_only: self.sharing_only,
            enabled: self.enabled,
            status_statistics_enabled: self.status_statistics_enabled,
            updated_at: self.updated_at.0,
        }
    }
}

#[derive(FromRow)]
struct ConsoleChannelRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    api_format: String,
    connector_kind: String,
    provider_managed: bool,
    name: String,
    base_url: String,
    enabled: bool,
    supports_websocket: bool,
    supports_standalone_web_search: bool,
    auto_disabled: bool,
    auto_disabled_reason: Option<String>,
    auto_disable_allowed: bool,
    billing_multiplier: SqliteUnitPrice,
    proxy_id: Option<SqliteUuid>,
    config_template_id: Option<SqliteUuid>,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    credential_id: Option<SqliteUuid>,
    available_models: String,
    test_model: Option<String>,
    test_pricing_model_id: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ConsoleChannelRow {
    fn into_view(self) -> Result<ControlPlaneChannel, RepositoryError> {
        Ok(ControlPlaneChannel {
            id: self.id.0,
            channel_group_id: self.channel_group_id.0,
            api_format: self.api_format,
            connector_kind: self.connector_kind,
            provider_managed: self.provider_managed,
            name: self.name,
            base_url: self.base_url,
            enabled: self.enabled,
            supports_websocket: self.supports_websocket,
            supports_standalone_web_search: self.supports_standalone_web_search,
            auto_disabled: self.auto_disabled,
            auto_disabled_reason: self.auto_disabled_reason,
            auto_disable_allowed: self.auto_disable_allowed,
            billing_multiplier: self.billing_multiplier.0,
            proxy_id: self.proxy_id.map(|value| value.0),
            config_template_id: self.config_template_id.map(|value| value.0),
            connect_timeout_ms: self.connect_timeout_ms,
            response_header_timeout_ms: self.response_header_timeout_ms,
            stream_idle_timeout_ms: self.stream_idle_timeout_ms,
            credential_id: self.credential_id.map(|id| id.0),
            available_models: string_list(&self.available_models)?,
            test_model: self.test_model,
            test_pricing_model_id: self.test_pricing_model_id.map(|value| value.0),
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneChannelDetailRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    api_format: String,
    connector_kind: String,
    provider_managed: bool,
    name: String,
    base_url: String,
    enabled: bool,
    supports_websocket: bool,
    supports_standalone_web_search: bool,
    auto_disabled: bool,
    auto_disabled_reason: Option<String>,
    auto_disable_allowed: bool,
    billing_multiplier: SqliteUnitPrice,
    proxy_id: Option<SqliteUuid>,
    config_template_id: Option<SqliteUuid>,
    override_document: String,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    credential_id: Option<SqliteUuid>,
    available_models: String,
    test_model: Option<String>,
    test_pricing_model_id: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneChannelDetailRow {
    fn into_view(self) -> Result<ControlPlaneChannelDetail, RepositoryError> {
        Ok(ControlPlaneChannelDetail {
            id: self.id.0,
            channel_group_id: self.channel_group_id.0,
            api_format: self.api_format,
            connector_kind: self.connector_kind,
            provider_managed: self.provider_managed,
            name: self.name,
            base_url: self.base_url,
            enabled: self.enabled,
            supports_websocket: self.supports_websocket,
            supports_standalone_web_search: self.supports_standalone_web_search,
            auto_disabled: self.auto_disabled,
            auto_disabled_reason: self.auto_disabled_reason,
            auto_disable_allowed: self.auto_disable_allowed,
            billing_multiplier: self.billing_multiplier.0,
            proxy_id: self.proxy_id.map(|value| value.0),
            config_template_id: self.config_template_id.map(|value| value.0),
            override_document: json_column(&self.override_document)?,
            connect_timeout_ms: self.connect_timeout_ms,
            response_header_timeout_ms: self.response_header_timeout_ms,
            stream_idle_timeout_ms: self.stream_idle_timeout_ms,
            credential_id: self.credential_id.map(|id| id.0),
            available_models: string_list(&self.available_models)?,
            test_model: self.test_model,
            test_pricing_model_id: self.test_pricing_model_id.map(|value| value.0),
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ConsoleProtocolRuleRow {
    id: SqliteUuid,
    model_rule_id: SqliteUuid,
    api_format: String,
    model_enabled: bool,
    description: Option<String>,
    enabled: bool,
    updated_at: SqliteTimestamp,
}

impl ConsoleProtocolRuleRow {
    fn into_view(
        self,
        routing_tiers: Vec<ModelRuleRoutingTier>,
        groups: &[ControlPlaneChannelGroup],
        channels: &[ControlPlaneChannel],
    ) -> ControlPlaneModelProtocolRule {
        let row = ControlPlaneModelProtocolRuleRow {
            id: self.id.0,
            model_rule_id: self.model_rule_id.0,
            api_format: self.api_format,
            model_enabled: self.model_enabled,
            description: self.description,
            routing_tiers: Json(routing_tiers),
            enabled: self.enabled,
            updated_at: self.updated_at.0,
        };
        let ControlPlaneModelProtocolRuleRow {
            id,
            model_rule_id,
            api_format,
            model_enabled,
            description,
            routing_tiers,
            enabled,
            updated_at,
        } = row;
        let routing_tiers = routing_tiers.0;
        let mut target_candidate_count = 0;
        let mut model_capable_candidate_count = 0;
        let mut active_candidate_count = 0;
        for candidate in routing_tiers.iter().flat_map(|tier| &tier.candidates) {
            target_candidate_count += 1;
            let Some(channel) = channels
                .iter()
                .find(|channel| channel.id == candidate.channel_id)
            else {
                continue;
            };
            if channel.api_format != api_format
                || !channel.available_models.contains(&candidate.upstream_model)
            {
                continue;
            }
            model_capable_candidate_count += 1;
            let group_enabled = groups
                .iter()
                .find(|group| group.id == channel.channel_group_id)
                .is_some_and(|group| group.enabled);
            if channel.enabled && !channel.auto_disabled && group_enabled {
                active_candidate_count += 1;
            }
        }
        let routing_status = if !model_enabled {
            ModelRuleRoutingStatus::ModelDisabled
        } else if routing_tiers.is_empty() {
            ModelRuleRoutingStatus::Draft
        } else if !enabled {
            ModelRuleRoutingStatus::Disabled
        } else if active_candidate_count > 0 {
            ModelRuleRoutingStatus::Ready
        } else if model_capable_candidate_count > 0 {
            ModelRuleRoutingStatus::TemporarilyUnavailable
        } else {
            ModelRuleRoutingStatus::Disconnected
        };
        ControlPlaneModelProtocolRule {
            id,
            model_rule_id,
            api_format,
            description,
            routing_tiers,
            enabled,
            routing_status,
            target_candidate_count,
            model_capable_candidate_count,
            active_candidate_count,
            updated_at,
        }
    }
}

#[derive(FromRow)]
struct ControlPlaneProxyRow {
    id: SqliteUuid,
    name: String,
    proxy_url: String,
    no_proxy_hosts: String,
    enabled: bool,
    credential_configured: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneProxyRow {
    fn into_view(self) -> Result<ControlPlaneProxy, RepositoryError> {
        Ok(ControlPlaneProxy {
            id: self.id.0,
            name: self.name,
            proxy_url: audit_proxy_url(&self.proxy_url),
            no_proxy_hosts: string_list(&self.no_proxy_hosts)?,
            enabled: self.enabled,
            credential_configured: self.credential_configured,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ControlPlaneConfigTemplateRow {
    id: SqliteUuid,
    name: String,
    description: Option<String>,
    api_format: Option<String>,
    enabled: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneConfigTemplateRow {
    fn into_view(self) -> ControlPlaneConfigTemplate {
        ControlPlaneConfigTemplate {
            id: self.id.0,
            name: self.name,
            description: self.description,
            api_format: self.api_format,
            enabled: self.enabled,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        }
    }
}

#[derive(FromRow)]
struct ControlPlaneConfigTemplateDetailRow {
    id: SqliteUuid,
    name: String,
    description: Option<String>,
    api_format: Option<String>,
    document: String,
    enabled: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ControlPlaneConfigTemplateDetailRow {
    fn into_view(self) -> Result<ControlPlaneConfigTemplateDetail, RepositoryError> {
        Ok(ControlPlaneConfigTemplateDetail {
            id: self.id.0,
            name: self.name,
            description: self.description,
            api_format: self.api_format,
            document: json_column(&self.document)?,
            enabled: self.enabled,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct ConsoleAuditLogRow {
    id: SqliteUuid,
    occurred_at: SqliteTimestamp,
    actor_user_id: Option<SqliteUuid>,
    actor_type: String,
    actor_role: Option<String>,
    action: String,
    object_type: String,
    object_id: SqliteUuid,
    before_redacted: Option<String>,
    after_redacted: Option<String>,
    correlation_id: Option<String>,
    reason: Option<String>,
}

impl ConsoleAuditLogRow {
    fn into_view(self) -> Result<ConsoleAuditLog, RepositoryError> {
        Ok(ConsoleAuditLog {
            id: self.id.0,
            occurred_at: self.occurred_at.0,
            actor_user_id: self.actor_user_id.map(|value| value.0),
            actor_type: self.actor_type,
            actor_role: self.actor_role,
            action: self.action,
            object_type: self.object_type,
            object_id: self.object_id.0,
            before_redacted: json_column_optional(self.before_redacted)?,
            after_redacted: json_column_optional(self.after_redacted)?,
            correlation_id: self.correlation_id,
            reason: self.reason,
        })
    }
}

#[derive(FromRow)]
struct ConsoleApiKeyRow {
    id: SqliteUuid,
    name: String,
    secret: String,
    status: String,
    expires_at: Option<SqliteTimestamp>,
    allowed_api_formats: String,
    permissions: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<SqliteAmount>,
    quota_used_amount: SqliteAmount,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ConsoleApiKeyRow {
    fn into_view(self) -> Result<ConsoleApiKey, RepositoryError> {
        Ok(ConsoleApiKey {
            id: self.id.0,
            name: self.name,
            secret: self.secret,
            status: self.status,
            expires_at: self.expires_at.map(|value| value.0),
            allowed_api_formats: string_list(&self.allowed_api_formats)?,
            permissions: string_list(&self.permissions)?,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            requests_per_minute: self.requests_per_minute,
            max_concurrent_requests: self.max_concurrent_requests,
            quota_limit_amount: self.quota_limit_amount.map(|value| value.0),
            quota_used_amount: self.quota_used_amount.0,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        })
    }
}

// ---------------------------------------------------------------------------
// Channel deletion impact
// ---------------------------------------------------------------------------

impl SqliteControlPlaneRepository {
    pub async fn channel_group_deletion_impact(
        &self,
        id: Uuid,
    ) -> Result<ChannelDeletionImpact, RepositoryError> {
        self.deletion_impact(ChannelDeletionTarget::Group(id)).await
    }

    pub async fn channel_deletion_impact(
        &self,
        id: Uuid,
    ) -> Result<ChannelDeletionImpact, RepositoryError> {
        self.deletion_impact(ChannelDeletionTarget::Channel(id))
            .await
    }

    async fn deletion_impact(
        &self,
        target: ChannelDeletionTarget,
    ) -> Result<ChannelDeletionImpact, RepositoryError> {
        let mut connection = self.read().await?;
        let mut transaction = connection.begin().await?;
        let plan = load_channel_deletion_plan(&mut transaction, target).await?;
        transaction.commit().await?;
        Ok(plan.impact)
    }
}

async fn load_channel_deletion_plan(
    transaction: &mut Transaction<'_, Sqlite>,
    target: ChannelDeletionTarget,
) -> Result<ChannelDeletionPlan, RepositoryError> {
    let connection = &mut **transaction;
    let (resource_type, root) = match target {
        ChannelDeletionTarget::Group(id) => (
            "channel_group",
            sqlx::query_as::<_, DeletionImpactRootRowRow>(
                "SELECT id,name,updated_at,connector_kind,NULL AS channel_group_id \
                 FROM channel_groups WHERE id=? AND deleted_at IS NULL",
            )
            .bind(SqliteUuid(id))
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(RepositoryError::NotFound)?
            .into_row(),
        ),
        ChannelDeletionTarget::Channel(id) => (
            "channel",
            sqlx::query_as::<_, DeletionImpactRootRowRow>(
                "SELECT c.id,c.name,c.updated_at,g.connector_kind,c.channel_group_id \
                 FROM channels AS c \
                 JOIN channel_groups AS g ON g.id=c.channel_group_id AND g.deleted_at IS NULL \
                 WHERE c.id=? AND c.deleted_at IS NULL",
            )
            .bind(SqliteUuid(id))
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(RepositoryError::NotFound)?
            .into_row(),
        ),
    };
    if root.connector_kind != "openai_compatible" {
        return Err(RepositoryError::ProviderManagedResource);
    }

    let deleted_group_ids = match target {
        ChannelDeletionTarget::Group(_) => vec![root.id],
        ChannelDeletionTarget::Channel(_) => Vec::new(),
    };
    let channels = match target {
        ChannelDeletionTarget::Group(_) => {
            sqlx::query_as::<_, DeletionImpactChannelRowRow>(
                "SELECT id,channel_group_id,name,available_models,updated_at \
                 FROM channels WHERE channel_group_id=? AND deleted_at IS NULL ORDER BY id",
            )
            .bind(SqliteUuid(root.id))
            .fetch_all(&mut *connection)
            .await?
        }
        ChannelDeletionTarget::Channel(_) => {
            sqlx::query_as::<_, DeletionImpactChannelRowRow>(
                "SELECT id,channel_group_id,name,available_models,updated_at \
                 FROM channels WHERE id=? AND deleted_at IS NULL",
            )
            .bind(SqliteUuid(root.id))
            .fetch_all(&mut *connection)
            .await?
        }
    }
    .into_iter()
    .map(DeletionImpactChannelRowRow::into_row)
    .collect::<Result<Vec<_>, _>>()?;
    let deleted_channel_ids = channels
        .iter()
        .map(|channel| channel.id)
        .collect::<Vec<_>>();

    let deleted_channel_array = uuid_array_text(&deleted_channel_ids)?;
    let rule_rows = sqlx::query_as::<_, DeletionImpactRuleRowRow>(
        "SELECT rule.id,rule.model_routing_profile_id AS model_rule_id, \
                model.source_model_id AS client_model,rule.api_format,rule.enabled,rule.updated_at \
         FROM model_rules AS rule \
         JOIN model_routing_profiles AS profile ON profile.id=rule.model_routing_profile_id \
         JOIN models AS model ON model.id=profile.model_id AND model.deleted_at IS NULL \
         WHERE EXISTS ( \
             SELECT 1 \
             FROM model_rule_routing_candidates AS candidate \
             WHERE candidate.model_rule_id=rule.id \
               AND EXISTS (SELECT 1 FROM json_each(?) AS deleted \
                           WHERE deleted.value=candidate.channel_id)) \
         ORDER BY rule.id",
    )
    .bind(&deleted_channel_array)
    .fetch_all(&mut *connection)
    .await?;
    let deleted_groups = deleted_group_ids.iter().copied().collect::<HashSet<_>>();
    let deleted_channels = deleted_channel_ids.iter().copied().collect::<HashSet<_>>();
    let channel_rows = channels
        .iter()
        .map(|channel| (channel.id, channel))
        .collect::<HashMap<_, _>>();
    let mut affected_rules = Vec::new();
    let mut rule_impacts = Vec::new();
    for row in rule_rows {
        let id = row.id.0;
        let mut state = row.into_row()?;
        state.routing_tiers = load_rule_routing_tiers(connection, id).await?;
        if let Some(impact) =
            deletion_impact_for_rule(&state, &deleted_groups, &deleted_channels, &channel_rows)
        {
            affected_rules.push(state);
            rule_impacts.push(impact);
        }
    }

    let deleted_group_array = uuid_array_text(&deleted_group_ids)?;
    let api_keys = sqlx::query_as::<_, DeletionImpactApiKeyRowRow>(
        "SELECT id,name,allowed_group_ids,allowed_channel_ids,updated_at \
         FROM api_keys WHERE deleted_at IS NULL \
           AND (EXISTS (SELECT 1 FROM json_each(?) AS target \
                        JOIN json_each(api_keys.allowed_group_ids) AS allowed \
                          ON allowed.value=target.value) \
                OR EXISTS (SELECT 1 FROM json_each(?) AS target \
                           JOIN json_each(api_keys.allowed_channel_ids) AS allowed \
                             ON allowed.value=target.value)) \
         ORDER BY id",
    )
    .bind(&deleted_group_array)
    .bind(&deleted_channel_array)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(DeletionImpactApiKeyRowRow::into_row)
    .collect::<Result<Vec<_>, _>>()?;
    let api_key_policies = sqlx::query_as::<_, DeletionImpactApiKeyPolicyRowRow>(
        "SELECT id,name,allowed_group_ids,allowed_channel_ids,updated_at \
         FROM api_key_policies \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       JOIN json_each(api_key_policies.allowed_group_ids) AS allowed \
                         ON allowed.value=target.value) \
            OR EXISTS (SELECT 1 FROM json_each(?) AS target \
                       JOIN json_each(api_key_policies.allowed_channel_ids) AS allowed \
                         ON allowed.value=target.value) \
         ORDER BY id",
    )
    .bind(&deleted_group_array)
    .bind(&deleted_channel_array)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(DeletionImpactApiKeyPolicyRowRow::into_row)
    .collect::<Result<Vec<_>, _>>()?;
    let quota_visibility = sqlx::query_as::<_, DeletionImpactVisibilityRowRow>(
        "SELECT visibility.user_group_id,group_record.name AS user_group_name, \
                visibility.channel_group_id \
         FROM user_group_codex_quota_visibility AS visibility \
         JOIN user_groups AS group_record ON group_record.id=visibility.user_group_id \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=visibility.channel_group_id) \
         ORDER BY visibility.user_group_id,visibility.channel_group_id",
    )
    .bind(&deleted_group_array)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(DeletionImpactVisibilityRowRow::into_row)
    .collect::<Vec<_>>();

    let fingerprint = DeletionImpactFingerprint {
        version: 1,
        resource_type,
        root: &root,
        channels: &channels,
        model_protocol_rules: &affected_rules,
        api_keys: &api_keys,
        api_key_policies: &api_key_policies,
        quota_visibility: &quota_visibility,
    };
    let serialized = serde_json::to_vec(&fingerprint).map_err(|_| RepositoryError::Validation)?;
    let confirmation_token = format!("v1.{}", sha256_hex(Sha256::digest(serialized)));
    let affected_rule_ids = rule_impacts.iter().map(|rule| rule.id).collect();
    let impact = ChannelDeletionImpact {
        resource_type: resource_type.into(),
        resource_id: root.id,
        confirmation_token,
        channels: channels
            .iter()
            .map(|channel| DeletionImpactChannel {
                id: channel.id,
                channel_group_id: channel.channel_group_id,
                name: channel.name.clone(),
            })
            .collect(),
        model_protocol_rules: rule_impacts,
        api_keys: api_keys
            .iter()
            .map(|key| DeletionImpactApiKey {
                id: key.id,
                name: key.name.clone(),
            })
            .collect(),
        api_key_policies: api_key_policies
            .iter()
            .map(|policy| DeletionImpactApiKeyPolicy {
                id: policy.id,
                name: policy.name.clone(),
            })
            .collect(),
        quota_visibility_user_groups: quota_visibility
            .iter()
            .map(|visibility| DeletionImpactUserGroup {
                id: visibility.user_group_id,
                name: visibility.user_group_name.clone(),
            })
            .collect(),
    };
    Ok(ChannelDeletionPlan {
        impact,
        root_updated_at: root.updated_at,
        deleted_group_ids,
        deleted_channel_ids,
        affected_rule_ids,
    })
}

fn uuid_array_text(values: &[Uuid]) -> Result<String, RepositoryError> {
    json_text(&values.iter().map(Uuid::to_string).collect::<Vec<_>>())
}

#[derive(FromRow)]
struct DeletionImpactRootRowRow {
    id: SqliteUuid,
    name: String,
    updated_at: SqliteTimestamp,
    connector_kind: String,
    channel_group_id: Option<SqliteUuid>,
}

impl DeletionImpactRootRowRow {
    fn into_row(self) -> DeletionImpactRootRow {
        DeletionImpactRootRow {
            id: self.id.0,
            name: self.name,
            updated_at: self.updated_at.0,
            connector_kind: self.connector_kind,
            channel_group_id: self.channel_group_id.map(|value| value.0),
        }
    }
}

#[derive(FromRow)]
struct DeletionImpactChannelRowRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    name: String,
    available_models: String,
    updated_at: SqliteTimestamp,
}

impl DeletionImpactChannelRowRow {
    fn into_row(self) -> Result<DeletionImpactChannelRow, RepositoryError> {
        Ok(DeletionImpactChannelRow {
            id: self.id.0,
            channel_group_id: self.channel_group_id.0,
            name: self.name,
            available_models: string_list(&self.available_models)?,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct DeletionImpactRuleRowRow {
    id: SqliteUuid,
    model_rule_id: SqliteUuid,
    client_model: String,
    api_format: String,
    enabled: bool,
    updated_at: SqliteTimestamp,
}

impl DeletionImpactRuleRowRow {
    fn into_row(self) -> Result<DeletionImpactRuleState, RepositoryError> {
        Ok(DeletionImpactRuleState {
            id: self.id.0,
            model_rule_id: self.model_rule_id.0,
            client_model: self.client_model,
            api_format: self.api_format,
            enabled: self.enabled,
            updated_at: self.updated_at.0,
            routing_tiers: Vec::new(),
        })
    }
}

#[derive(FromRow)]
struct DeletionImpactApiKeyRowRow {
    id: SqliteUuid,
    name: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    updated_at: SqliteTimestamp,
}

impl DeletionImpactApiKeyRowRow {
    fn into_row(self) -> Result<DeletionImpactApiKeyRow, RepositoryError> {
        Ok(DeletionImpactApiKeyRow {
            id: self.id.0,
            name: self.name,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct DeletionImpactApiKeyPolicyRowRow {
    id: SqliteUuid,
    name: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    updated_at: SqliteTimestamp,
}

impl DeletionImpactApiKeyPolicyRowRow {
    fn into_row(self) -> Result<DeletionImpactApiKeyPolicyRow, RepositoryError> {
        Ok(DeletionImpactApiKeyPolicyRow {
            id: self.id.0,
            name: self.name,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            updated_at: self.updated_at.0,
        })
    }
}

#[derive(FromRow)]
struct DeletionImpactVisibilityRowRow {
    user_group_id: SqliteUuid,
    user_group_name: String,
    channel_group_id: SqliteUuid,
}

impl DeletionImpactVisibilityRowRow {
    fn into_row(self) -> DeletionImpactVisibilityRow {
        DeletionImpactVisibilityRow {
            user_group_id: self.user_group_id.0,
            user_group_name: self.user_group_name,
            channel_group_id: self.channel_group_id.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Self-service API-key options and ownership
// ---------------------------------------------------------------------------

impl SqliteControlPlaneRepository {
    pub async fn own_api_key_options(
        &self,
        user_id: Uuid,
    ) -> Result<SelfApiKeyOptions, RepositoryError> {
        let mut reader = self.read().await?;
        let policy = sqlx::query_as::<_, SelfApiKeyPolicyRow>(
            "SELECT p.id,p.name,p.allowed_group_ids,p.allowed_channel_ids,p.enabled \
             FROM users AS u \
             JOIN user_groups AS g ON g.id=u.user_group_id AND g.deleted_at IS NULL \
             JOIN api_key_policies AS p \
               ON p.id=COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
             WHERE u.id=? AND u.status='active' AND u.deleted_at IS NULL",
        )
        .bind(SqliteUuid(user_id))
        .fetch_optional(&mut *reader)
        .await?
        .map(SelfApiKeyPolicyRow::into_policy)
        .transpose()?;
        let sharing_credentials = sqlx::query_as::<_, SharingCredentialOptionRow>(
            "SELECT s.credential_id,s.id AS sharing_group_id,s.name,s.enabled, \
                    COALESCE((SELECT json_group_array(p.channel_id) \
                              FROM codex_oauth_credential_channels AS p \
                              WHERE p.credential_id=s.credential_id \
                              ORDER BY p.api_format),'[]') AS channel_ids, \
                    COALESCE((SELECT json_group_array(p.api_format) \
                              FROM codex_oauth_credential_channels AS p \
                              WHERE p.credential_id=s.credential_id \
                              ORDER BY p.api_format),'[]') AS api_formats \
             FROM codex_sharing_groups AS s \
             JOIN users AS u ON u.id=? AND u.status='active' AND u.deleted_at IS NULL \
                              AND u.is_system=0 \
             WHERE EXISTS (SELECT 1 FROM json_each(s.seats) AS seat \
                           WHERE seat.value=CAST(? AS TEXT)) \
             ORDER BY s.name,s.id",
        )
        .bind(SqliteUuid(user_id))
        .bind(user_id.to_string())
        .fetch_all(&mut *reader)
        .await?
        .into_iter()
        .map(SharingCredentialOptionRow::into_view)
        .collect::<Result<Vec<_>, _>>()?;
        if sharing_credentials.is_empty() {
            ensure_optional_policy_enabled(policy.as_ref())?;
        }

        let (groups, channels) = if let Some(policy) = policy.as_ref().filter(|p| p.enabled) {
            let allowed_groups = uuid_array_text(&policy.allowed_group_ids)?;
            let groups = sqlx::query_as::<_, SelfApiKeyGroupOptionRow>(
                "SELECT g.id,g.name,g.api_format,g.enabled \
                 FROM channel_groups AS g \
                 WHERE EXISTS (SELECT 1 FROM json_each(?) AS allowed \
                               WHERE allowed.value=g.id) \
                   AND g.deleted_at IS NULL AND g.sharing_only=0 \
                   AND EXISTS (SELECT 1 FROM channels AS candidate \
                     WHERE candidate.channel_group_id=g.id AND candidate.deleted_at IS NULL \
                       AND NOT EXISTS ( \
                         SELECT 1 \
                         FROM codex_oauth_credential_channels AS option_projection \
                         JOIN codex_oauth_credentials AS option_credential \
                           ON option_credential.channel_id=option_projection.credential_id \
                         JOIN codex_sharing_groups AS s \
                           ON s.credential_id=option_credential.channel_id \
                           OR (COALESCE(option_credential.account_id,'')=s.provider_account_id \
                               AND option_credential.user_id=s.provider_user_id) \
                         WHERE option_projection.channel_id=candidate.id \
                           AND option_credential.deleted_at IS NULL)) \
                 ORDER BY g.api_format,g.name,g.id",
            )
            .bind(&allowed_groups)
            .fetch_all(&mut *reader)
            .await?
            .into_iter()
            .map(SelfApiKeyGroupOptionRow::into_view)
            .collect();
            let allowed_channels = uuid_array_text(&policy.allowed_channel_ids)?;
            let channels = sqlx::query_as::<_, SelfApiKeyChannelOptionRow>(
                "SELECT c.id,c.channel_group_id,g.name AS channel_group_name, \
                        g.enabled AS channel_group_enabled,c.api_format,c.name,c.enabled, \
                        c.auto_disabled \
                 FROM channels AS c \
                 JOIN channel_groups AS g ON g.id=c.channel_group_id AND g.deleted_at IS NULL \
                 WHERE c.deleted_at IS NULL AND g.sharing_only=0 \
                   AND (EXISTS (SELECT 1 FROM json_each(?) AS allowed \
                                WHERE allowed.value=c.channel_group_id) \
                        OR EXISTS (SELECT 1 FROM json_each(?) AS allowed \
                                   WHERE allowed.value=c.id)) \
                   AND NOT EXISTS ( \
                       SELECT 1 \
                       FROM codex_oauth_credential_channels AS option_projection \
                       JOIN codex_oauth_credentials AS option_credential \
                         ON option_credential.channel_id=option_projection.credential_id \
                       JOIN codex_sharing_groups AS s \
                         ON s.credential_id=option_credential.channel_id \
                         OR (COALESCE(option_credential.account_id,'')=s.provider_account_id \
                             AND option_credential.user_id=s.provider_user_id) \
                       WHERE option_projection.channel_id=c.id \
                         AND option_credential.deleted_at IS NULL) \
                 ORDER BY c.api_format,g.name,c.name,c.id",
            )
            .bind(&allowed_groups)
            .bind(&allowed_channels)
            .fetch_all(&mut *reader)
            .await?
            .into_iter()
            .map(SelfApiKeyChannelOptionRow::into_view)
            .collect();
            (groups, channels)
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(SelfApiKeyOptions {
            policy_id: policy.as_ref().map(|p| p.id),
            policy_name: policy.as_ref().map(|p| p.name.clone()),
            policy_enabled: policy.as_ref().is_some_and(|p| p.enabled),
            sharing_credentials,
            groups,
            channels,
        })
    }
}

#[derive(FromRow)]
struct SelfApiKeyPolicyRow {
    id: SqliteUuid,
    name: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    enabled: bool,
}

impl SelfApiKeyPolicyRow {
    fn into_policy(self) -> Result<SelfApiKeyPolicy, RepositoryError> {
        Ok(SelfApiKeyPolicy {
            id: self.id.0,
            name: self.name,
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
            enabled: self.enabled,
        })
    }
}

#[derive(FromRow)]
struct SharingCredentialOptionRow {
    credential_id: SqliteUuid,
    sharing_group_id: SqliteUuid,
    name: String,
    enabled: bool,
    channel_ids: String,
    api_formats: String,
}

impl SharingCredentialOptionRow {
    fn into_view(self) -> Result<SelfApiKeySharingCredentialOption, RepositoryError> {
        Ok(SelfApiKeySharingCredentialOption {
            credential_id: self.credential_id.0,
            sharing_group_id: self.sharing_group_id.0,
            name: self.name,
            enabled: self.enabled,
            channel_ids: uuid_list(&self.channel_ids)?,
            api_formats: string_list(&self.api_formats)?,
        })
    }
}

#[derive(FromRow)]
struct SelfApiKeyGroupOptionRow {
    id: SqliteUuid,
    name: String,
    api_format: String,
    enabled: bool,
}

impl SelfApiKeyGroupOptionRow {
    fn into_view(self) -> SelfApiKeyGroupOption {
        SelfApiKeyGroupOption {
            id: self.id.0,
            name: self.name,
            api_format: self.api_format,
            enabled: self.enabled,
        }
    }
}

#[derive(FromRow)]
struct SelfApiKeyChannelOptionRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    channel_group_name: String,
    channel_group_enabled: bool,
    api_format: String,
    name: String,
    enabled: bool,
    auto_disabled: bool,
}

impl SelfApiKeyChannelOptionRow {
    fn into_view(self) -> SelfApiKeyChannelOption {
        SelfApiKeyChannelOption {
            id: self.id.0,
            channel_group_id: self.channel_group_id.0,
            channel_group_name: self.channel_group_name,
            channel_group_enabled: self.channel_group_enabled,
            api_format: self.api_format,
            name: self.name,
            enabled: self.enabled,
            auto_disabled: self.auto_disabled,
        }
    }
}

fn ensure_optional_policy_enabled(
    policy: Option<&SelfApiKeyPolicy>,
) -> Result<(), RepositoryError> {
    match policy {
        Some(policy) if policy.enabled => Ok(()),
        Some(_) => Err(RepositoryError::DefaultApiKeyPolicyDisabled),
        None => Err(RepositoryError::DefaultApiKeyPolicyRequired),
    }
}

fn same_uuid_set(left: &[Uuid], right: &[Uuid]) -> bool {
    left.len() == right.len()
        && left.iter().copied().collect::<HashSet<_>>()
            == right.iter().copied().collect::<HashSet<_>>()
}

fn validate_target_lists(
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
    allow_empty: bool,
) -> Result<(), RepositoryError> {
    if (!allow_empty && allowed_group_ids.is_empty() && allowed_channel_ids.is_empty())
        || allowed_group_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != allowed_group_ids.len()
        || allowed_channel_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != allowed_channel_ids.len()
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn validate_api_key_limits(
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<Decimal>,
) -> Result<(), RepositoryError> {
    if requests_per_minute.is_some_and(|value| value <= 0)
        || max_concurrent_requests.is_some_and(|value| value <= 0)
        || quota_limit_amount.is_some_and(|value| value.is_sign_negative())
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn validate_self_api_key_input(
    name: &str,
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<Decimal>,
    allow_empty_targets: bool,
) -> Result<(), RepositoryError> {
    if name.trim().is_empty() {
        return Err(RepositoryError::Validation);
    }
    validate_target_lists(allowed_group_ids, allowed_channel_ids, allow_empty_targets)?;
    validate_api_key_limits(
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_admin_api_key_input(
    name: &str,
    allowed_api_formats: &[String],
    permissions: &[String],
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<Decimal>,
) -> Result<(), RepositoryError> {
    if name.trim().is_empty()
        || allowed_api_formats.is_empty()
        || permissions.is_empty()
        || allowed_api_formats.iter().collect::<HashSet<_>>().len() != allowed_api_formats.len()
        || permissions.iter().collect::<HashSet<_>>().len() != permissions.len()
    {
        return Err(RepositoryError::Validation);
    }
    validate_target_lists(allowed_group_ids, allowed_channel_ids, false)?;
    validate_api_key_limits(
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
    )
}

// ---------------------------------------------------------------------------
// Prepared control-plane changes
// ---------------------------------------------------------------------------

/// Audit attribution for one prepared change, mirroring the PostgreSQL
/// `Audit` classification.
pub(super) enum AuditKind {
    Admin(Uuid),
    SelfService(Uuid),
    System,
    Reload(Uuid),
    None,
}

/// One validated, uncommitted control-plane change. The transaction owns the
/// SQLite write lock, so the value is independent of the repository borrow.
/// Dropping it rolls back; callers must read and validate
/// [`Self::runtime_records`] before committing and publish the compiled
/// snapshot only after `commit` succeeds.
pub struct SqlitePreparedControlPlaneChange {
    transaction: Transaction<'static, Sqlite>,
    mutations: Vec<MutationResult>,
    audit: AuditKind,
}

impl SqlitePreparedControlPlaneChange {
    pub async fn runtime_records(&mut self) -> Result<RuntimeConfigRecords, RepositoryError> {
        SqliteControlPlaneRepository::load_runtime_transaction(&mut self.transaction).await
    }

    pub async fn commit(mut self) -> Result<(Vec<MutationResult>, Uuid), RepositoryError> {
        let correlation_id = Uuid::new_v4();
        for mutation in &self.mutations {
            match self.audit {
                AuditKind::Admin(actor) => {
                    insert_user_audit(
                        &mut self.transaction,
                        actor,
                        "admin",
                        mutation,
                        correlation_id,
                    )
                    .await?
                }
                AuditKind::SelfService(actor) => {
                    insert_user_audit(
                        &mut self.transaction,
                        actor,
                        "user",
                        mutation,
                        correlation_id,
                    )
                    .await?
                }
                AuditKind::System => {
                    insert_system_audit(&mut self.transaction, mutation, correlation_id).await?
                }
                AuditKind::Reload(_) | AuditKind::None => {}
            }
        }
        if let AuditKind::Reload(actor) = self.audit {
            insert_manual_reload_audit(&mut self.transaction, actor, correlation_id).await?;
        }
        self.transaction.commit().await?;
        for mutation in &mut self.mutations {
            mutation.correlation_id = Some(correlation_id);
        }
        Ok((self.mutations, correlation_id))
    }

    /// Rolls the prepared change back while keeping any error the caller
    /// already observed; used when snapshot compilation fails before commit.
    pub async fn rollback(self) -> Result<(), RepositoryError> {
        self.transaction.rollback().await?;
        Ok(())
    }
}

impl SqliteControlPlaneRepository {
    pub(super) fn prepared(
        &self,
        transaction: Transaction<'static, Sqlite>,
        mutations: Vec<MutationResult>,
        audit: AuditKind,
    ) -> SqlitePreparedControlPlaneChange {
        SqlitePreparedControlPlaneChange {
            transaction,
            mutations,
            audit,
        }
    }

    pub(super) async fn admin_write(
        &self,
        actor: Uuid,
    ) -> Result<Transaction<'static, Sqlite>, RepositoryError> {
        let mut transaction = self.write().await?;
        if !active_admin_exists(&mut transaction, actor).await? {
            return Err(RepositoryError::InvalidActor);
        }
        Ok(transaction)
    }

    async fn self_service_write(
        &self,
        actor: Uuid,
    ) -> Result<Transaction<'static, Sqlite>, RepositoryError> {
        let mut transaction = self.write().await?;
        if !active_user_exists(&mut transaction, actor).await? {
            return Err(RepositoryError::InvalidActor);
        }
        Ok(transaction)
    }

    pub async fn verify_active_admin(&self, actor: Uuid) -> Result<(), RepositoryError> {
        self.admin_write(actor).await?.rollback().await?;
        Ok(())
    }

    pub async fn prepare_manual_reload(
        &self,
        actor: Uuid,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let transaction = self.admin_write(actor).await?;
        Ok(self.prepared(transaction, Vec::new(), AuditKind::Reload(actor)))
    }

    pub async fn prepare_user_settings(
        &self,
        user_id: Uuid,
        input: UserSettingsInput,
    ) -> Result<(UserSettingsView, SqlitePreparedControlPlaneChange), RepositoryError> {
        let mut transaction = self.write().await?;
        let settings = update_user_settings(&mut transaction, user_id, input)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        Ok((
            settings,
            self.prepared(transaction, Vec::new(), AuditKind::None),
        ))
    }

    pub async fn prepare_mutation(
        &self,
        actor: Uuid,
        mutation: ControlPlaneMutation,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let result = apply_control_plane_mutation(&mut transaction, mutation).await?;
        Ok(self.prepared(transaction, vec![result], AuditKind::Admin(actor)))
    }

    pub async fn prepare_channels_batch(
        &self,
        actor: Uuid,
        input: ChannelBatchUpdateInput,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = update_channels_batch(&mut transaction, input).await?;
        Ok(self.prepared(transaction, results, AuditKind::Admin(actor)))
    }

    pub async fn prepare_users_batch(
        &self,
        actor: Uuid,
        input: UserBatchUpdateInput,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = update_users_batch(&mut transaction, actor, input).await?;
        Ok(self.prepared(transaction, results, AuditKind::Admin(actor)))
    }

    pub async fn prepare_own_api_key_create(
        &self,
        actor: Uuid,
        input: SelfApiKeyCreate,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = create_own_api_key(&mut transaction, actor, input).await?;
        Ok(self.prepared(transaction, vec![result], AuditKind::SelfService(actor)))
    }

    pub async fn prepare_own_api_key_update(
        &self,
        actor: Uuid,
        id: Uuid,
        input: SelfApiKeyUpdate,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result =
            update_own_api_key(&mut transaction, actor, id, input, expected_updated_at).await?;
        Ok(self.prepared(transaction, vec![result], AuditKind::SelfService(actor)))
    }

    pub async fn prepare_own_api_key_revoke(
        &self,
        actor: Uuid,
        id: Uuid,
        reason: String,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = revoke_own_api_key(&mut transaction, actor, id, reason).await?;
        Ok(self.prepared(transaction, vec![result], AuditKind::SelfService(actor)))
    }

    pub async fn prepare_own_api_key_delete(
        &self,
        actor: Uuid,
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = delete_own_api_key(&mut transaction, actor, id, expected_updated_at).await?;
        Ok(self.prepared(transaction, vec![result], AuditKind::SelfService(actor)))
    }

    pub async fn prepare_catalog_models(
        &self,
        actor: Uuid,
        inputs: Vec<crate::persistence::SyncedModelInput>,
    ) -> Result<SqlitePreparedControlPlaneChange, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = apply_catalog_models(&mut transaction, inputs).await?;
        Ok(self.prepared(transaction, results, AuditKind::Admin(actor)))
    }

    pub async fn prepare_channel_disable(
        &self,
        channel_id: Uuid,
        trigger: &AutomaticDisableTrigger,
    ) -> Result<Option<SqlitePreparedControlPlaneChange>, RepositoryError> {
        let mut transaction = self.write().await?;
        let Some(result) =
            automatically_disable_channel(&mut transaction, channel_id, trigger).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        Ok(Some(self.prepared(
            transaction,
            vec![result],
            AuditKind::System,
        )))
    }

    pub async fn prepare_channel_recovery(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<SqlitePreparedControlPlaneChange>, RepositoryError> {
        let mut transaction = self.write().await?;
        let Some(result) = automatically_recover_channel(&mut transaction, channel_id).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        Ok(Some(self.prepared(
            transaction,
            vec![result],
            AuditKind::System,
        )))
    }
}

async fn active_user_exists(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<bool, RepositoryError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users \
         WHERE id=? AND status='active' AND deleted_at IS NULL)",
    )
    .bind(SqliteUuid(id))
    .fetch_one(&mut **transaction)
    .await?)
}

async fn active_admin_exists(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<bool, RepositoryError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users \
         WHERE id=? AND status='active' AND role='admin' AND deleted_at IS NULL)",
    )
    .bind(SqliteUuid(id))
    .fetch_one(&mut **transaction)
    .await?)
}

async fn update_user_settings(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
    input: UserSettingsInput,
) -> Result<Option<UserSettingsView>, RepositoryError> {
    let row = sqlx::query_as::<_, UserSettingsRow>(
        "UPDATE users SET websocket_enabled=?,updated_at=ag_now() \
         WHERE id=? AND status='active' AND deleted_at IS NULL \
         RETURNING websocket_enabled,updated_at",
    )
    .bind(input.websocket_enabled)
    .bind(SqliteUuid(user_id))
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(row.map(UserSettingsRow::into_view))
}

// ---------------------------------------------------------------------------
// Audit projections
// ---------------------------------------------------------------------------

pub(super) async fn insert_user_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    actor: Uuid,
    actor_role: &str,
    mutation: &MutationResult,
    correlation_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
          before_redacted,after_redacted,correlation_id,reason) \
         VALUES (?,?,'user',?,?,?,?,?,?,?,?)",
    )
    .bind(SqliteUuid(Uuid::new_v4()))
    .bind(SqliteUuid(actor))
    .bind(actor_role)
    .bind(mutation.action)
    .bind(mutation.object_type)
    .bind(SqliteUuid(mutation.id))
    .bind(value_to_text(&mutation.before_redacted)?)
    .bind(value_to_text(&mutation.after_redacted)?)
    .bind(correlation_id.to_string())
    .bind(&mutation.reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_system_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    mutation: &MutationResult,
    correlation_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_type,action,object_type,object_id,before_redacted,after_redacted, \
          correlation_id,reason) \
         VALUES (?,'system',?,?,?,?,?,?,?)",
    )
    .bind(SqliteUuid(Uuid::new_v4()))
    .bind(mutation.action)
    .bind(mutation.object_type)
    .bind(SqliteUuid(mutation.id))
    .bind(value_to_text(&mutation.before_redacted)?)
    .bind(value_to_text(&mutation.after_redacted)?)
    .bind(correlation_id.to_string())
    .bind(&mutation.reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_manual_reload_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    actor: Uuid,
    correlation_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
          before_redacted,after_redacted,correlation_id) \
         VALUES (?,?,'user','admin','reload','runtime_config',?,?,?,?)",
    )
    .bind(SqliteUuid(Uuid::new_v4()))
    .bind(SqliteUuid(actor))
    .bind(SqliteUuid(Uuid::nil()))
    .bind("{}")
    .bind("{}")
    .bind(correlation_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct KeyAuditRow {
    id: SqliteUuid,
    user_id: SqliteUuid,
    name: String,
    status: String,
    expires_at: Option<SqliteTimestamp>,
    allowed_api_formats: String,
    permissions: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<SqliteAmount>,
    quota_used_amount: SqliteAmount,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl KeyAuditRow {
    fn into_value(self) -> Result<Value, RepositoryError> {
        Ok(json!({
            "id": self.id.0,
            "user_id": self.user_id.0,
            "name": self.name,
            "status": self.status,
            "expires_at": self.expires_at.map(|value| value.0),
            "allowed_api_formats": string_list(&self.allowed_api_formats)?,
            "permissions": string_list(&self.permissions)?,
            "allowed_group_ids": uuid_list(&self.allowed_group_ids)?,
            "allowed_channel_ids": uuid_list(&self.allowed_channel_ids)?,
            "requests_per_minute": self.requests_per_minute,
            "max_concurrent_requests": self.max_concurrent_requests,
            "quota_limit_amount": json_option_decimal(self.quota_limit_amount.map(|value| value.0)),
            "quota_used_amount": json_decimal(self.quota_used_amount.0),
            "deleted_at": self.deleted_at.map(|value| value.0),
            "deleted_by": self.deleted_by.map(|value| value.0),
            "created_at": self.created_at.0,
            "updated_at": self.updated_at.0,
        }))
    }
}

const KEY_AUDIT_COLUMNS: &str = "id,user_id,name,status,expires_at,allowed_api_formats,permissions, \
     allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests, \
     quota_limit_amount,quota_used_amount,deleted_at,deleted_by,created_at,updated_at";

async fn key_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, KeyAuditRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {KEY_AUDIT_COLUMNS} FROM api_keys WHERE id=? AND is_system=0"
    )))
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    row.into_value()
}

async fn key_audit_for_user(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    user_id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, KeyAuditRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {KEY_AUDIT_COLUMNS} FROM api_keys WHERE id=? AND user_id=? AND is_system=0"
    )))
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(user_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    row.into_value()
}

#[derive(FromRow)]
struct UserAuditRow {
    id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    password_hash: Option<String>,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
    user_group_id: SqliteUuid,
    user_group_system_role: Option<String>,
    default_api_key_policy_id: Option<SqliteUuid>,
    effective_api_key_policy_id: Option<SqliteUuid>,
    websocket_enabled: bool,
    balance_amount: SqliteAmount,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn user_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, UserAuditRow>(
        "SELECT u.id,u.email,u.display_name,u.role,u.status,u.password_hash, \
                u.password_change_required,u.temporary_password_expires_at,u.user_group_id, \
                g.system_role AS user_group_system_role,u.default_api_key_policy_id, \
                COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
                    AS effective_api_key_policy_id, \
                u.websocket_enabled,u.balance_amount,u.deleted_at,u.deleted_by, \
                u.created_at,u.updated_at \
         FROM users AS u \
         JOIN user_groups AS g ON g.id=u.user_group_id \
         WHERE u.id=? AND u.is_system=0",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "email": row.email,
        "display_name": row.display_name,
        "role": row.role,
        "status": row.status,
        "can_reissue_invitation": row.password_hash.is_none()
            && row.email.is_some()
            && matches!(row.status.as_str(), "invited" | "suspended" | "disabled"),
        "password_change_required": row.password_change_required,
        "temporary_password_expires_at": row.temporary_password_expires_at.map(|value| value.0),
        "user_group_id": row.user_group_id.0,
        "user_group_system_role": row.user_group_system_role,
        "default_api_key_policy_id": row.default_api_key_policy_id.map(|value| value.0),
        "effective_api_key_policy_id": row.effective_api_key_policy_id.map(|value| value.0),
        "websocket_enabled": row.websocket_enabled,
        "balance_amount": json_decimal(row.balance_amount.0),
        "deleted_at": row.deleted_at.map(|value| value.0),
        "deleted_by": row.deleted_by.map(|value| value.0),
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

async fn user_group_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, UserGroupAuditRow>(
        "SELECT g.id,g.name,g.description,g.default_api_key_policy_id, \
                COALESCE((SELECT json_group_array(visibility.channel_group_id) \
                          FROM user_group_codex_quota_visibility AS visibility \
                          WHERE visibility.user_group_id=g.id \
                          ORDER BY visibility.channel_group_id),'[]') \
                    AS visible_codex_quota_group_ids, \
                g.filter_fast_mode,g.system_role, \
                count(CASE WHEN u.deleted_at IS NULL AND u.is_system=0 THEN u.id END) AS member_count, \
                g.deleted_at,g.deleted_by,g.created_at,g.updated_at \
         FROM user_groups AS g \
         LEFT JOIN users AS u ON u.user_group_id=g.id \
         WHERE g.id=? GROUP BY g.id",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "name": row.name,
        "description": row.description,
        "default_api_key_policy_id": row.default_api_key_policy_id.map(|value| value.0),
        "visible_codex_quota_group_ids": uuid_list(&row.visible_codex_quota_group_ids)?,
        "filter_fast_mode": row.filter_fast_mode,
        "system_role": row.system_role,
        "member_count": row.member_count,
        "deleted_at": row.deleted_at.map(|value| value.0),
        "deleted_by": row.deleted_by.map(|value| value.0),
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct UserGroupAuditRow {
    id: SqliteUuid,
    name: String,
    description: Option<String>,
    default_api_key_policy_id: Option<SqliteUuid>,
    visible_codex_quota_group_ids: String,
    filter_fast_mode: bool,
    system_role: Option<String>,
    member_count: i64,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn model_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, ModelAuditRow>(
        "SELECT id,source_model_id,display_name,provider_name,enabled,price_unit_tokens, \
                input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price, \
                price_effective_at,advanced_billing,last_synced_at,deleted_at,deleted_by, \
                created_at,updated_at \
         FROM models WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "source_model_id": row.source_model_id,
        "display_name": row.display_name,
        "provider_name": row.provider_name,
        "enabled": row.enabled,
        "price_unit_tokens": row.price_unit_tokens,
        "input_unit_price": json_decimal(row.input_unit_price.0),
        "cached_input_unit_price": json_decimal(row.cached_input_unit_price.0),
        "cache_write_unit_price": json_decimal(row.cache_write_unit_price.0),
        "output_unit_price": json_decimal(row.output_unit_price.0),
        "price_effective_at": row.price_effective_at.0,
        "advanced_billing": json_column(&row.advanced_billing)?,
        "last_synced_at": row.last_synced_at.map(|value| value.0),
        "deleted_at": row.deleted_at.map(|value| value.0),
        "deleted_by": row.deleted_by.map(|value| value.0),
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct ModelAuditRow {
    id: SqliteUuid,
    source_model_id: String,
    display_name: String,
    provider_name: Option<String>,
    enabled: bool,
    price_unit_tokens: i64,
    input_unit_price: SqliteUnitPrice,
    cached_input_unit_price: SqliteUnitPrice,
    cache_write_unit_price: SqliteUnitPrice,
    output_unit_price: SqliteUnitPrice,
    price_effective_at: SqliteTimestamp,
    advanced_billing: String,
    last_synced_at: Option<SqliteTimestamp>,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn group_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, ChannelGroupAuditRow>(
        "SELECT id,name,api_format,connector_kind,connector_pool_id,request_compression, \
                sharing_only,enabled,status_statistics_enabled,deleted_at,deleted_by, \
                created_at,updated_at \
         FROM channel_groups WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let mut value = json!({
        "id": row.id.0,
        "name": row.name,
        "api_format": row.api_format,
        "connector_kind": row.connector_kind,
        "connector_pool_id": row.connector_pool_id.map(|value| value.0),
        "request_compression": row.request_compression,
        "sharing_only": row.sharing_only,
        "enabled": row.enabled,
        "status_statistics_enabled": row.status_statistics_enabled,
        "deleted_at": row.deleted_at.map(|value| value.0),
        "deleted_by": row.deleted_by.map(|value| value.0),
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    });
    // Codex pools expose every sibling projection so an audit of one group
    // records the shared sharing-only state.
    if let Some(pool_id) = row.connector_pool_id {
        let siblings = sqlx::query_as::<_, ChannelGroupAuditRow>(
            "SELECT id,name,api_format,connector_kind,connector_pool_id,request_compression, \
                    sharing_only,enabled,status_statistics_enabled,deleted_at,deleted_by, \
                    created_at,updated_at \
             FROM channel_groups WHERE connector_pool_id=? ORDER BY api_format",
        )
        .bind(SqliteUuid(pool_id.0))
        .fetch_all(&mut **transaction)
        .await?;
        let sibling_values = siblings
            .into_iter()
            .map(ChannelGroupAuditRow::into_value)
            .collect::<Result<Vec<_>, _>>()?;
        value["connector_pool_groups"] = Value::Array(sibling_values);
    }
    Ok(value)
}

#[derive(FromRow)]
struct ChannelGroupAuditRow {
    id: SqliteUuid,
    name: String,
    api_format: String,
    connector_kind: String,
    connector_pool_id: Option<SqliteUuid>,
    request_compression: String,
    sharing_only: bool,
    enabled: bool,
    status_statistics_enabled: bool,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ChannelGroupAuditRow {
    fn into_value(self) -> Result<Value, RepositoryError> {
        Ok(json!({
            "id": self.id.0,
            "name": self.name,
            "api_format": self.api_format,
            "connector_kind": self.connector_kind,
            "connector_pool_id": self.connector_pool_id.map(|value| value.0),
            "request_compression": self.request_compression,
            "sharing_only": self.sharing_only,
            "enabled": self.enabled,
            "status_statistics_enabled": self.status_statistics_enabled,
            "deleted_at": self.deleted_at.map(|value| value.0),
            "deleted_by": self.deleted_by.map(|value| value.0),
            "created_at": self.created_at.0,
            "updated_at": self.updated_at.0,
        }))
    }
}

async fn channel_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    // Audit snapshots stay allowlisted even though authorized detail reads
    // expose the stored credential and transform document for editing.
    let row = sqlx::query_as::<_, ChannelAuditRow>(
        "SELECT id,channel_group_id,api_format,name,base_url,enabled,supports_websocket, \
                supports_standalone_web_search,auto_disabled,auto_disabled_reason, \
                auto_disable_allowed,billing_multiplier,proxy_id,config_template_id, \
                connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms, \
                credential_id, \
                available_models,test_model,test_pricing_model_id,deleted_at,deleted_by, \
                created_at,updated_at \
         FROM channels WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "channel_group_id": row.channel_group_id.0,
        "api_format": row.api_format,
        "name": row.name,
        "base_url": row.base_url,
        "enabled": row.enabled,
        "supports_websocket": row.supports_websocket,
        "supports_standalone_web_search": row.supports_standalone_web_search,
        "auto_disabled": row.auto_disabled,
        "auto_disabled_reason": row.auto_disabled_reason,
        "auto_disable_allowed": row.auto_disable_allowed,
        "billing_multiplier": json_decimal(row.billing_multiplier.0),
        "proxy_id": row.proxy_id.map(|value| value.0),
        "config_template_id": row.config_template_id.map(|value| value.0),
        "connect_timeout_ms": row.connect_timeout_ms,
        "response_header_timeout_ms": row.response_header_timeout_ms,
        "stream_idle_timeout_ms": row.stream_idle_timeout_ms,
        "credential_id": row.credential_id.map(|id| id.0),
        "available_models": string_list(&row.available_models)?,
        "test_model": row.test_model,
        "test_pricing_model_id": row.test_pricing_model_id.map(|value| value.0),
        "deleted_at": row.deleted_at.map(|value| value.0),
        "deleted_by": row.deleted_by.map(|value| value.0),
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct ChannelAuditRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    api_format: String,
    name: String,
    base_url: String,
    enabled: bool,
    supports_websocket: bool,
    supports_standalone_web_search: bool,
    auto_disabled: bool,
    auto_disabled_reason: Option<String>,
    auto_disable_allowed: bool,
    billing_multiplier: SqliteUnitPrice,
    proxy_id: Option<SqliteUuid>,
    config_template_id: Option<SqliteUuid>,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    credential_id: Option<SqliteUuid>,
    available_models: String,
    test_model: Option<String>,
    test_pricing_model_id: Option<SqliteUuid>,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn model_routing_profile_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, RoutingProfileAuditRow>(
        "SELECT profile.id,profile.model_id,model.source_model_id AS client_model, \
                profile.created_at,profile.updated_at \
         FROM model_routing_profiles AS profile \
         JOIN models AS model ON model.id=profile.model_id \
         WHERE profile.id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "model_id": row.model_id.0,
        "client_model": row.client_model,
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct RoutingProfileAuditRow {
    id: SqliteUuid,
    model_id: SqliteUuid,
    client_model: String,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn rule_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, RuleAuditRow>(
        "SELECT id,model_routing_profile_id,api_format,enabled,description,created_at,updated_at \
         FROM model_rules WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let routing_tiers = load_rule_routing_tiers(transaction, id).await?;
    Ok(json!({
        "id": row.id.0,
        "model_routing_profile_id": row.model_routing_profile_id.0,
        "api_format": row.api_format,
        "enabled": row.enabled,
        "description": row.description,
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
        "routing_tiers": routing_tiers,
    }))
}

#[derive(FromRow)]
struct RuleAuditRow {
    id: SqliteUuid,
    model_routing_profile_id: SqliteUuid,
    api_format: String,
    enabled: bool,
    description: Option<String>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn proxy_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, ProxyAuditRow>(
        "SELECT id,name,proxy_url,no_proxy_hosts,enabled, \
                (username IS NOT NULL OR password IS NOT NULL) AS credential_configured, \
                created_at,updated_at \
         FROM proxies WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "name": row.name,
        "proxy_url": audit_proxy_url(&row.proxy_url),
        "no_proxy_hosts": string_list(&row.no_proxy_hosts)?,
        "enabled": row.enabled,
        "credential_configured": row.credential_configured,
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct ProxyAuditRow {
    id: SqliteUuid,
    name: String,
    proxy_url: String,
    no_proxy_hosts: String,
    enabled: bool,
    credential_configured: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn config_template_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, ConfigTemplateAuditRow>(
        "SELECT id,name,description,enabled,created_at,updated_at \
         FROM config_templates WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "name": row.name,
        "description": row.description,
        "enabled": row.enabled,
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct ConfigTemplateAuditRow {
    id: SqliteUuid,
    name: String,
    description: Option<String>,
    enabled: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

async fn api_key_policy_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, ApiKeyPolicyAuditRow>(
        "SELECT id,name,allowed_group_ids,allowed_channel_ids,enabled,created_at,updated_at \
         FROM api_key_policies WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(json!({
        "id": row.id.0,
        "name": row.name,
        "allowed_group_ids": uuid_list(&row.allowed_group_ids)?,
        "allowed_channel_ids": uuid_list(&row.allowed_channel_ids)?,
        "enabled": row.enabled,
        "created_at": row.created_at.0,
        "updated_at": row.updated_at.0,
    }))
}

#[derive(FromRow)]
struct ApiKeyPolicyAuditRow {
    id: SqliteUuid,
    name: String,
    allowed_group_ids: String,
    allowed_channel_ids: String,
    enabled: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

// ---------------------------------------------------------------------------
// Self-service API-key writes
// ---------------------------------------------------------------------------

async fn load_optional_self_api_key_policy(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
) -> Result<Option<SelfApiKeyPolicy>, RepositoryError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users \
         WHERE id=? AND status='active' AND deleted_at IS NULL AND is_system=0)",
    )
    .bind(SqliteUuid(user_id))
    .fetch_one(&mut **transaction)
    .await?;
    if !exists {
        return Err(RepositoryError::DefaultApiKeyPolicyRequired);
    }
    let row = sqlx::query_as::<_, SelfApiKeyPolicyRow>(
        "SELECT p.id,p.name,p.allowed_group_ids,p.allowed_channel_ids,p.enabled \
         FROM users AS u \
         JOIN user_groups AS g ON g.id=u.user_group_id AND g.deleted_at IS NULL \
         JOIN api_key_policies AS p \
           ON p.id=COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
         WHERE u.id=? AND u.status='active' AND u.deleted_at IS NULL",
    )
    .bind(SqliteUuid(user_id))
    .fetch_optional(&mut **transaction)
    .await?;
    row.map(SelfApiKeyPolicyRow::into_policy).transpose()
}

async fn load_self_api_key_sharing_access(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
) -> Result<SelfApiKeySharingAccess, RepositoryError> {
    let rows = sqlx::query_as::<_, SharingAccessRow>(
        "SELECT projection.channel_id, \
                max(candidate.channel_id=s.credential_id \
                    AND EXISTS (SELECT 1 FROM json_each(s.seats) AS seat \
                                WHERE seat.value=CAST(? AS TEXT))) AS owned \
         FROM codex_sharing_groups AS s \
         JOIN codex_oauth_credentials AS candidate \
           ON candidate.channel_id=s.credential_id \
           OR (COALESCE(candidate.account_id,'')=s.provider_account_id \
               AND candidate.user_id=s.provider_user_id) \
         JOIN codex_oauth_credential_channels AS projection \
           ON projection.credential_id=candidate.channel_id \
         WHERE candidate.deleted_at IS NULL \
         GROUP BY projection.channel_id",
    )
    .bind(user_id.to_string())
    .fetch_all(&mut **transaction)
    .await?;
    let mut access = SelfApiKeySharingAccess::default();
    for row in rows {
        access.protected_channels.insert(row.channel_id.0);
        if row.owned {
            access.owned_channels.insert(row.channel_id.0);
        }
    }
    Ok(access)
}

#[derive(FromRow)]
struct SharingAccessRow {
    channel_id: SqliteUuid,
    owned: bool,
}

async fn resolve_self_api_key_targets(
    transaction: &mut Transaction<'static, Sqlite>,
    selected_group_ids: &[Uuid],
    selected_channel_ids: &[Uuid],
    policy: Option<&SelfApiKeyPolicy>,
    sharing: &SelfApiKeySharingAccess,
) -> Result<Vec<String>, RepositoryError> {
    let selected_groups = uuid_array_text(selected_group_ids)?;
    let groups = sqlx::query_as::<_, ApiKeyTargetGroupRow>(
        "SELECT g.id,g.api_format,g.sharing_only, \
                EXISTS (SELECT 1 FROM channels AS candidate \
                  WHERE candidate.channel_group_id=g.id AND candidate.deleted_at IS NULL \
                    AND NOT EXISTS ( \
                      SELECT 1 \
                      FROM codex_oauth_credential_channels AS projection \
                      JOIN codex_oauth_credentials AS credential \
                        ON credential.channel_id=projection.credential_id \
                      JOIN codex_sharing_groups AS s \
                        ON s.credential_id=credential.channel_id \
                        OR (COALESCE(credential.account_id,'')=s.provider_account_id \
                            AND credential.user_id=s.provider_user_id) \
                      WHERE projection.channel_id=candidate.id \
                        AND credential.deleted_at IS NULL)) AS has_ordinary_channel \
         FROM channel_groups AS g \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS selected \
                       WHERE selected.value=g.id) \
           AND g.deleted_at IS NULL",
    )
    .bind(&selected_groups)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(ApiKeyTargetGroupRow::into_target)
    .collect::<Result<Vec<_>, _>>()?;
    let selected_channels = uuid_array_text(selected_channel_ids)?;
    let channels = sqlx::query_as::<_, ApiKeyTargetChannelRow>(
        "SELECT c.id,c.channel_group_id,c.api_format,g.sharing_only \
         FROM channels AS c \
         JOIN channel_groups AS g ON g.id=c.channel_group_id AND g.deleted_at IS NULL \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS selected \
                       WHERE selected.value=c.id) \
           AND c.deleted_at IS NULL",
    )
    .bind(&selected_channels)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(ApiKeyTargetChannelRow::into_target)
    .collect::<Vec<_>>();
    if groups.len() != selected_group_ids.len() || channels.len() != selected_channel_ids.len() {
        return Err(RepositoryError::ApiKeyTargetNotAllowed);
    }

    if groups
        .iter()
        .any(|group| group.sharing_only || !group.has_ordinary_channel)
        || channels.iter().any(|channel| {
            sharing.protected_channels.contains(&channel.id)
                && !sharing.owned_channels.contains(&channel.id)
        })
    {
        return Err(RepositoryError::ApiKeyTargetNotAllowed);
    }
    let ordinary_groups = groups.iter().collect::<Vec<_>>();
    let ordinary_channels = channels
        .iter()
        .filter(|channel| !sharing.owned_channels.contains(&channel.id))
        .collect::<Vec<_>>();
    if !ordinary_groups.is_empty() || !ordinary_channels.is_empty() {
        ensure_optional_policy_enabled(policy)?;
        let policy = policy.expect("enabled policy was required");
        let allowed_groups = policy
            .allowed_group_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let allowed_channels = policy
            .allowed_channel_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if ordinary_groups
            .iter()
            .any(|group| !allowed_groups.contains(&group.id))
            || ordinary_channels.iter().any(|channel| {
                channel.sharing_only
                    || (!allowed_groups.contains(&channel.channel_group_id)
                        && !allowed_channels.contains(&channel.id))
            })
        {
            return Err(RepositoryError::ApiKeyTargetNotAllowed);
        }
    }

    let mut formats = BTreeSet::new();
    formats.extend(groups.into_iter().map(|group| group.api_format));
    formats.extend(channels.into_iter().map(|channel| channel.api_format));
    if formats.is_empty() {
        return Err(RepositoryError::Validation);
    }
    Ok(formats.into_iter().collect())
}

#[derive(FromRow)]
struct ApiKeyTargetGroupRow {
    id: SqliteUuid,
    api_format: String,
    sharing_only: bool,
    has_ordinary_channel: bool,
}

impl ApiKeyTargetGroupRow {
    fn into_target(self) -> Result<ApiKeyTargetGroup, RepositoryError> {
        Ok(ApiKeyTargetGroup {
            id: self.id.0,
            api_format: self.api_format,
            sharing_only: self.sharing_only,
            has_ordinary_channel: self.has_ordinary_channel,
        })
    }
}

#[derive(FromRow)]
struct ApiKeyTargetChannelRow {
    id: SqliteUuid,
    channel_group_id: SqliteUuid,
    api_format: String,
    sharing_only: bool,
}

impl ApiKeyTargetChannelRow {
    fn into_target(self) -> ApiKeyTargetChannel {
        ApiKeyTargetChannel {
            id: self.id.0,
            channel_group_id: self.channel_group_id.0,
            api_format: self.api_format,
            sharing_only: self.sharing_only,
        }
    }
}

async fn create_own_api_key(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
    input: SelfApiKeyCreate,
) -> Result<MutationResult, RepositoryError> {
    validate_self_api_key_input(
        &input.name,
        &input.allowed_group_ids,
        &input.allowed_channel_ids,
        input.requests_per_minute,
        input.max_concurrent_requests,
        input.quota_limit_amount,
        false,
    )?;
    let policy = load_optional_self_api_key_policy(transaction, user_id).await?;
    let sharing = load_self_api_key_sharing_access(transaction, user_id).await?;
    let allowed_api_formats = resolve_self_api_key_targets(
        transaction,
        &input.allowed_group_ids,
        &input.allowed_channel_ids,
        policy.as_ref(),
        &sharing,
    )
    .await?;
    let id = Uuid::new_v4();
    let secret = generate_api_key_secret();
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,expires_at,allowed_api_formats,permissions, \
          allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests, \
          quota_limit_amount) \
         VALUES (?,?,?,?,'active',?,?,?,?,?,?,?,?) RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(user_id))
    .bind(&input.name)
    .bind(&secret)
    .bind(input.expires_at.map(SqliteTimestamp))
    .bind(json_text(&allowed_api_formats)?)
    .bind(json_text(&["proxy", "models.read"])?)
    .bind(value_to_text(&json!(input.allowed_group_ids))?)
    .bind(value_to_text(&json!(input.allowed_channel_ids))?)
    .bind(input.requests_per_minute)
    .bind(input.max_concurrent_requests)
    .bind(input.quota_limit_amount.map(amount_24_8).transpose()?)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(MutationResult {
        id,
        object_type: "api_key",
        action: "self_create",
        before_redacted: json!({}),
        after_redacted: key_audit(transaction, id).await?,
        created_secret: Some(secret),
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn update_own_api_key(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
    id: Uuid,
    input: SelfApiKeyUpdate,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(RepositoryError::Validation);
    }
    let current = sqlx::query_as::<_, SelfApiKeyCurrentRow>(
        "SELECT allowed_group_ids,allowed_channel_ids FROM api_keys \
         WHERE id=? AND user_id=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(user_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let current = current.into_current()?;
    let targets_changed = !same_uuid_set(&current.allowed_group_ids, &input.allowed_group_ids)
        || !same_uuid_set(&current.allowed_channel_ids, &input.allowed_channel_ids);
    validate_self_api_key_input(
        &input.name,
        &input.allowed_group_ids,
        &input.allowed_channel_ids,
        input.requests_per_minute,
        input.max_concurrent_requests,
        input.quota_limit_amount,
        !targets_changed,
    )?;
    let allowed_api_formats = if targets_changed {
        let policy = load_optional_self_api_key_policy(transaction, user_id).await?;
        let sharing = load_self_api_key_sharing_access(transaction, user_id).await?;
        Some(
            resolve_self_api_key_targets(
                transaction,
                &input.allowed_group_ids,
                &input.allowed_channel_ids,
                policy.as_ref(),
                &sharing,
            )
            .await?,
        )
    } else {
        None
    };
    let before = key_audit_for_user(transaction, id, user_id).await?;
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE api_keys SET \
             name=?,status=?,expires_at=?, \
             allowed_api_formats=CASE WHEN ? THEN ? ELSE allowed_api_formats END, \
             allowed_group_ids=?,allowed_channel_ids=?,requests_per_minute=?, \
             max_concurrent_requests=?,quota_limit_amount=?,updated_at=ag_now() \
         WHERE id=? AND user_id=? AND updated_at=? \
           AND status <> 'revoked' AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(&input.name)
    .bind(&input.status)
    .bind(input.expires_at.map(SqliteTimestamp))
    .bind(targets_changed)
    .bind(json_text(&allowed_api_formats.unwrap_or_default())?)
    .bind(value_to_text(&json!(input.allowed_group_ids))?)
    .bind(value_to_text(&json!(input.allowed_channel_ids))?)
    .bind(input.requests_per_minute)
    .bind(input.max_concurrent_requests)
    .bind(input.quota_limit_amount.map(amount_24_8).transpose()?)
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(user_id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "api_key",
        action: "self_update",
        before_redacted: before,
        after_redacted: key_audit_for_user(transaction, id, user_id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

#[derive(FromRow)]
struct SelfApiKeyCurrentRow {
    allowed_group_ids: String,
    allowed_channel_ids: String,
}

impl SelfApiKeyCurrentRow {
    fn into_current(self) -> Result<SelfApiKeyCurrent, RepositoryError> {
        Ok(SelfApiKeyCurrent {
            allowed_group_ids: uuid_list(&self.allowed_group_ids)?,
            allowed_channel_ids: uuid_list(&self.allowed_channel_ids)?,
        })
    }
}

async fn revoke_own_api_key(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
    id: Uuid,
    reason: String,
) -> Result<MutationResult, RepositoryError> {
    if reason.trim().is_empty() {
        return Err(RepositoryError::Validation);
    }
    let before = key_audit_for_user(transaction, id, user_id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE api_keys SET status='revoked',updated_at=ag_now() \
         WHERE id=? AND user_id=? AND status <> 'revoked' AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(user_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "api_key",
        action: "self_revoke",
        before_redacted: before,
        after_redacted: key_audit_for_user(transaction, id, user_id).await?,
        created_secret: None,
        reason: Some(reason),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn delete_own_api_key(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
    id: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    api_key_soft_delete(
        transaction,
        id,
        Some(user_id),
        user_id,
        expected_updated_at,
        "self_delete",
    )
    .await
}

async fn api_key_soft_delete(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    owner_id: Option<Uuid>,
    deleted_by: Uuid,
    expected_updated_at: DateTime<Utc>,
    action: &'static str,
) -> Result<MutationResult, RepositoryError> {
    let before = if let Some(owner_id) = owner_id {
        key_audit_for_user(transaction, id, owner_id).await?
    } else {
        key_audit(transaction, id).await?
    };
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let secret = deleted_api_key_secret(id);
    let updated_at = if let Some(owner_id) = owner_id {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE api_keys SET status='revoked',secret_value=?,deleted_at=ag_now(), \
                    deleted_by=?,updated_at=ag_now() \
             WHERE id=? AND user_id=? AND updated_at=? \
               AND deleted_at IS NULL AND is_system=0 \
             RETURNING updated_at",
        )
        .bind(&secret)
        .bind(SqliteUuid(deleted_by))
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(owner_id))
        .bind(SqliteTimestamp(expected_updated_at))
        .fetch_optional(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE api_keys SET status='revoked',secret_value=?,deleted_at=ag_now(), \
                    deleted_by=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL AND is_system=0 \
             RETURNING updated_at",
        )
        .bind(&secret)
        .bind(SqliteUuid(deleted_by))
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at))
        .fetch_optional(&mut **transaction)
        .await?
    }
    .ok_or(RepositoryError::Conflict)?;
    let after = if let Some(owner_id) = owner_id {
        key_audit_for_user(transaction, id, owner_id).await?
    } else {
        key_audit(transaction, id).await?
    };
    Ok(MutationResult {
        id,
        object_type: "api_key",
        action,
        before_redacted: before,
        after_redacted: after,
        created_secret: None,
        reason: Some("API key deleted and secret erased".into()),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

// ---------------------------------------------------------------------------
// Batch updates
// ---------------------------------------------------------------------------

async fn update_users_batch(
    transaction: &mut Transaction<'static, Sqlite>,
    actor: Uuid,
    input: UserBatchUpdateInput,
) -> Result<Vec<MutationResult>, RepositoryError> {
    const MAX_BATCH_SIZE: usize = 100;

    if input.items.is_empty()
        || input.items.len() > MAX_BATCH_SIZE
        || input.changes.is_empty()
        || input
            .changes
            .status
            .as_deref()
            .is_some_and(|status| !matches!(status, "active" | "suspended" | "disabled"))
        || input.changes.balance.as_ref().is_some_and(|balance| {
            balance.amount.is_sign_negative()
                || !matches!(balance.operation.as_str(), "set" | "increase" | "decrease")
        })
    {
        return Err(RepositoryError::Validation);
    }
    let mut ids = HashSet::with_capacity(input.items.len());
    if input.items.iter().any(|item| !ids.insert(item.id)) {
        return Err(RepositoryError::Validation);
    }
    if let Some(group_id) = input.changes.user_group_id {
        ensure_user_group_exists(transaction, group_id).await?;
    }
    if let Some(Some(policy_id)) = input.changes.default_api_key_policy_id {
        ensure_enabled_policy(transaction, policy_id).await?;
    }

    let balance_operation = input
        .changes
        .balance
        .as_ref()
        .map(|balance| balance.operation.clone());
    let balance_amount = input.changes.balance.as_ref().map(|balance| balance.amount);
    let policy_present = input.changes.default_api_key_policy_id.is_some();
    let policy_id = input.changes.default_api_key_policy_id.flatten();
    let mut results = Vec::with_capacity(input.items.len());

    for item in input.items {
        let before = user_audit(transaction, item.id).await?;
        if !before["deleted_at"].is_null() {
            return Err(RepositoryError::NotFound);
        }
        let current_updated_at: DateTime<Utc> =
            serde_json::from_value(before["updated_at"].clone())
                .map_err(|_| RepositoryError::Validation)?;
        if current_updated_at != item.updated_at {
            return Err(RepositoryError::Conflict);
        }
        let status_changed = input
            .changes
            .status
            .as_deref()
            .is_some_and(|status| before["status"].as_str() != Some(status));
        if let Some(next_status) = input.changes.status.as_deref() {
            validate_user_status_transition(
                before["status"]
                    .as_str()
                    .ok_or(RepositoryError::Validation)?,
                next_status,
                before["can_reissue_invitation"].as_bool() == Some(true),
            )?;
            if item.id == actor && next_status != "active" {
                return Err(RepositoryError::CannotDisableSelf);
            }
        }
        let current_balance =
            sqlx::query_scalar::<_, SqliteAmount>("SELECT balance_amount FROM users WHERE id=?")
                .bind(SqliteUuid(item.id))
                .fetch_one(&mut **transaction)
                .await?
                .0;
        let next_balance = match (balance_operation.as_deref(), balance_amount) {
            (Some("set"), Some(amount)) => Some(amount),
            (Some("increase"), Some(amount)) => Some(
                current_balance
                    .checked_add(amount)
                    .ok_or(RepositoryError::Validation)?,
            ),
            (Some("decrease"), Some(amount)) => Some(
                current_balance
                    .checked_sub(amount)
                    .ok_or(RepositoryError::Validation)?,
            ),
            _ => None,
        };
        let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE users SET \
                 status=COALESCE(?,status), \
                 balance_amount=COALESCE(?,balance_amount), \
                 user_group_id=COALESCE(?,user_group_id), \
                 default_api_key_policy_id=CASE WHEN ? THEN ? ELSE default_api_key_policy_id END, \
                 auth_version=auth_version+CASE WHEN ? THEN 1 ELSE 0 END, \
                 updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL AND is_system=0 \
             RETURNING updated_at",
        )
        .bind(&input.changes.status)
        .bind(next_balance.map(amount_24_8).transpose()?)
        .bind(input.changes.user_group_id.map(SqliteUuid))
        .bind(policy_present)
        .bind(policy_id.map(SqliteUuid))
        .bind(status_changed)
        .bind(SqliteUuid(item.id))
        .bind(SqliteTimestamp(item.updated_at))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        if status_changed {
            revoke_live_sessions(transaction, item.id).await?;
        }
        results.push(MutationResult {
            id: item.id,
            object_type: "user",
            action: "batch_update",
            before_redacted: before,
            after_redacted: user_audit(transaction, item.id).await?,
            created_secret: None,
            reason: None,
            updated_at: updated_at.0,
            correlation_id: None,
        });
    }
    Ok(results)
}

async fn update_channels_batch(
    transaction: &mut Transaction<'static, Sqlite>,
    input: ChannelBatchUpdateInput,
) -> Result<Vec<MutationResult>, RepositoryError> {
    const MAX_BATCH_SIZE: usize = 100;

    if input.items.is_empty()
        || input.items.len() > MAX_BATCH_SIZE
        || input.changes.is_empty()
        || input
            .changes
            .billing_multiplier
            .is_some_and(|multiplier| multiplier.is_sign_negative())
    {
        return Err(RepositoryError::Validation);
    }
    let mut ids = HashSet::with_capacity(input.items.len());
    if input.items.iter().any(|item| !ids.insert(item.id)) {
        return Err(RepositoryError::Validation);
    }

    let mut results = Vec::with_capacity(input.items.len());
    for item in input.items {
        if channel_is_provider_managed(transaction, item.id).await? {
            return Err(RepositoryError::Validation);
        }
        let before = channel_audit(transaction, item.id).await?;
        if !before["deleted_at"].is_null() {
            return Err(RepositoryError::NotFound);
        }
        let current_updated_at: DateTime<Utc> =
            serde_json::from_value(before["updated_at"].clone())
                .map_err(|_| RepositoryError::Validation)?;
        if current_updated_at != item.updated_at {
            return Err(RepositoryError::Conflict);
        }
        let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE channels SET \
                 enabled=COALESCE(?,enabled), \
                 auto_disable_allowed=COALESCE(?,auto_disable_allowed), \
                 billing_multiplier=COALESCE(?,billing_multiplier), \
                 updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL \
             RETURNING updated_at",
        )
        .bind(input.changes.enabled)
        .bind(input.changes.auto_disable_allowed)
        .bind(
            input
                .changes
                .billing_multiplier
                .map(amount_24_12)
                .transpose()?,
        )
        .bind(SqliteUuid(item.id))
        .bind(SqliteTimestamp(item.updated_at))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        results.push(MutationResult {
            id: item.id,
            object_type: "channel",
            action: "batch_update",
            before_redacted: before,
            after_redacted: channel_audit(transaction, item.id).await?,
            created_secret: None,
            reason: None,
            updated_at: updated_at.0,
            correlation_id: None,
        });
    }
    Ok(results)
}

async fn ensure_user_group_exists(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<(), RepositoryError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM user_groups WHERE id=? AND deleted_at IS NULL)",
    )
    .bind(SqliteUuid(id))
    .fetch_one(&mut **transaction)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

async fn ensure_enabled_policy(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<(), RepositoryError> {
    let enabled = sqlx::query_scalar::<_, bool>("SELECT enabled FROM api_key_policies WHERE id=?")
        .bind(SqliteUuid(id))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Validation)?;
    if enabled {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

async fn revoke_live_sessions(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "UPDATE user_sessions SET revoked_at=ag_now() \
         WHERE user_id=? AND revoked_at IS NULL",
    )
    .bind(SqliteUuid(user_id))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Administrative mutations
// ---------------------------------------------------------------------------

async fn apply_control_plane_mutation(
    transaction: &mut Transaction<'static, Sqlite>,
    mutation: ControlPlaneMutation,
) -> Result<MutationResult, RepositoryError> {
    match mutation {
        ControlPlaneMutation::SaveCodexSharing {
            id,
            input,
            expected_updated_at,
        } => save_codex_sharing(transaction, id, input, expected_updated_at).await,
        ControlPlaneMutation::CreateUser(input) => {
            user_create(transaction, Uuid::new_v4(), input).await
        }
        ControlPlaneMutation::UpdateUser {
            id,
            input,
            expected_updated_at,
        } => user_update(transaction, id, input, expected_updated_at).await,
        ControlPlaneMutation::DeleteUser {
            id,
            deleted_by,
            expected_updated_at,
        } => user_soft_delete(transaction, id, deleted_by, expected_updated_at).await,
        ControlPlaneMutation::CreateUserGroup(input) => {
            user_group_insert(transaction, Uuid::new_v4(), input, true, None).await
        }
        ControlPlaneMutation::UpdateUserGroup {
            id,
            input,
            expected_updated_at,
        } => user_group_insert(transaction, id, input, false, Some(expected_updated_at)).await,
        ControlPlaneMutation::DeleteUserGroup {
            id,
            deleted_by,
            expected_updated_at,
        } => user_group_soft_delete(transaction, id, deleted_by, expected_updated_at).await,
        ControlPlaneMutation::CreateModel(input) => {
            model_insert(transaction, Uuid::new_v4(), input, true, None).await
        }
        ControlPlaneMutation::UpdateModel {
            id,
            input,
            expected_updated_at,
        } => model_insert(transaction, id, input, false, Some(expected_updated_at)).await,
        ControlPlaneMutation::DeleteModel {
            id,
            deleted_by,
            expected_updated_at,
        } => model_soft_delete(transaction, id, deleted_by, expected_updated_at).await,
        ControlPlaneMutation::CreateApiKey(input) => {
            ensure_api_key_owner_exists(transaction, input.user_id).await?;
            validate_admin_api_key_input(
                &input.name,
                &input.allowed_api_formats,
                &input.permissions,
                &input.allowed_group_ids,
                &input.allowed_channel_ids,
                input.requests_per_minute,
                input.max_concurrent_requests,
                input.quota_limit_amount,
            )?;
            validate_policy_targets(
                transaction,
                &input.allowed_group_ids,
                &input.allowed_channel_ids,
            )
            .await?;
            let id = Uuid::new_v4();
            let secret = generate_api_key_secret();
            let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
                "INSERT INTO api_keys \
                 (id,user_id,name,secret_value,status,expires_at,allowed_api_formats,permissions, \
                  allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests, \
                  quota_limit_amount) \
                 VALUES (?,?,?,?,'active',?,?,?,?,?,?,?,?) RETURNING updated_at",
            )
            .bind(SqliteUuid(id))
            .bind(SqliteUuid(input.user_id))
            .bind(&input.name)
            .bind(&secret)
            .bind(input.expires_at.map(SqliteTimestamp))
            .bind(json_text(&input.allowed_api_formats)?)
            .bind(json_text(&input.permissions)?)
            .bind(value_to_text(&json!(input.allowed_group_ids))?)
            .bind(value_to_text(&json!(input.allowed_channel_ids))?)
            .bind(input.requests_per_minute)
            .bind(input.max_concurrent_requests)
            .bind(input.quota_limit_amount.map(amount_24_8).transpose()?)
            .fetch_one(&mut **transaction)
            .await?;
            Ok(MutationResult {
                id,
                object_type: "api_key",
                action: "create",
                before_redacted: json!({}),
                after_redacted: key_audit(transaction, id).await?,
                created_secret: Some(secret),
                reason: None,
                updated_at: updated_at.0,
                correlation_id: None,
            })
        }
        ControlPlaneMutation::CreateApiKeyPolicy(input) => {
            api_key_policy_insert(transaction, Uuid::new_v4(), input, true, None).await
        }
        ControlPlaneMutation::UpdateApiKeyPolicy {
            id,
            input,
            expected_updated_at,
        } => api_key_policy_insert(transaction, id, input, false, Some(expected_updated_at)).await,
        ControlPlaneMutation::UpdateApiKey {
            id,
            input,
            expected_updated_at,
        } => {
            validate_admin_api_key_input(
                &input.name,
                &input.allowed_api_formats,
                &input.permissions,
                &input.allowed_group_ids,
                &input.allowed_channel_ids,
                input.requests_per_minute,
                input.max_concurrent_requests,
                input.quota_limit_amount,
            )?;
            validate_policy_targets(
                transaction,
                &input.allowed_group_ids,
                &input.allowed_channel_ids,
            )
            .await?;
            let before = key_audit(transaction, id).await?;
            if !before["deleted_at"].is_null() {
                return Err(RepositoryError::NotFound);
            }
            let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
                "UPDATE api_keys SET name=?,status=?,expires_at=?,allowed_api_formats=?, \
                        permissions=?,allowed_group_ids=?,allowed_channel_ids=?, \
                        requests_per_minute=?,max_concurrent_requests=?,quota_limit_amount=?, \
                        updated_at=ag_now() \
                 WHERE id=? AND updated_at=? AND deleted_at IS NULL \
                   AND NOT (status='revoked' AND ? <> 'revoked') \
                 RETURNING updated_at",
            )
            .bind(&input.name)
            .bind(&input.status)
            .bind(input.expires_at.map(SqliteTimestamp))
            .bind(json_text(&input.allowed_api_formats)?)
            .bind(json_text(&input.permissions)?)
            .bind(value_to_text(&json!(input.allowed_group_ids))?)
            .bind(value_to_text(&json!(input.allowed_channel_ids))?)
            .bind(input.requests_per_minute)
            .bind(input.max_concurrent_requests)
            .bind(input.quota_limit_amount.map(amount_24_8).transpose()?)
            .bind(SqliteUuid(id))
            .bind(SqliteTimestamp(expected_updated_at))
            .bind(&input.status)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Conflict)?;
            Ok(MutationResult {
                id,
                object_type: "api_key",
                action: "update",
                before_redacted: before,
                after_redacted: key_audit(transaction, id).await?,
                created_secret: None,
                reason: None,
                updated_at: updated_at.0,
                correlation_id: None,
            })
        }
        ControlPlaneMutation::DeleteApiKey {
            id,
            deleted_by,
            expected_updated_at,
        } => {
            api_key_soft_delete(
                transaction,
                id,
                None,
                deleted_by,
                expected_updated_at,
                "delete",
            )
            .await
        }
        ControlPlaneMutation::RevokeApiKey { id, reason } => {
            if reason.trim().is_empty() {
                return Err(RepositoryError::Validation);
            }
            let before = key_audit(transaction, id).await?;
            if !before["deleted_at"].is_null() {
                return Err(RepositoryError::NotFound);
            }
            let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
                "UPDATE api_keys SET status='revoked',updated_at=ag_now() \
                 WHERE id=? AND status <> 'revoked' AND deleted_at IS NULL \
                 RETURNING updated_at",
            )
            .bind(SqliteUuid(id))
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Conflict)?;
            Ok(MutationResult {
                id,
                object_type: "api_key",
                action: "revoke",
                before_redacted: before,
                after_redacted: key_audit(transaction, id).await?,
                created_secret: None,
                reason: Some(reason),
                updated_at: updated_at.0,
                correlation_id: None,
            })
        }
        ControlPlaneMutation::CreateGroup(input) => {
            group_insert(transaction, Uuid::new_v4(), input, true, None).await
        }
        ControlPlaneMutation::UpdateGroup {
            id,
            input,
            expected_updated_at,
        } => group_insert(transaction, id, input, false, Some(expected_updated_at)).await,
        ControlPlaneMutation::DeleteGroup {
            id,
            deleted_by,
            expected_updated_at,
            confirmation_token,
        } => {
            channel_resource_soft_delete(
                transaction,
                ChannelDeletionTarget::Group(id),
                deleted_by,
                expected_updated_at,
                &confirmation_token,
            )
            .await
        }
        ControlPlaneMutation::CreateChannel(input) => {
            channel_insert(transaction, Uuid::new_v4(), input, true, None).await
        }
        ControlPlaneMutation::UpdateChannel {
            id,
            input,
            expected_updated_at,
        } => channel_insert(transaction, id, input, false, Some(expected_updated_at)).await,
        ControlPlaneMutation::DeleteChannel {
            id,
            deleted_by,
            expected_updated_at,
            confirmation_token,
        } => {
            channel_resource_soft_delete(
                transaction,
                ChannelDeletionTarget::Channel(id),
                deleted_by,
                expected_updated_at,
                &confirmation_token,
            )
            .await
        }
        ControlPlaneMutation::RecoverChannel {
            id,
            expected_updated_at,
        } => channel_recover(transaction, id, expected_updated_at).await,
        ControlPlaneMutation::CreateRule(input) => {
            model_routing_profile_insert(transaction, Uuid::new_v4(), input).await
        }
        ControlPlaneMutation::CreateProtocolRule {
            model_rule_id,
            input,
        } => model_protocol_rule_create(transaction, model_rule_id, Uuid::new_v4(), input).await,
        ControlPlaneMutation::UpdateProtocolRule {
            model_rule_id,
            id,
            input,
            expected_updated_at,
        } => {
            model_protocol_rule_update(transaction, model_rule_id, id, input, expected_updated_at)
                .await
        }
        ControlPlaneMutation::CreateUpstreamCredential(input) => {
            super::upstream_credentials::save(transaction, Uuid::new_v4(), input, None).await
        }
        ControlPlaneMutation::UpdateUpstreamCredential {
            id,
            input,
            expected_updated_at,
        } => {
            super::upstream_credentials::save(transaction, id, input, Some(expected_updated_at))
                .await
        }
        ControlPlaneMutation::DeleteUpstreamCredential {
            id,
            expected_updated_at,
        } => super::upstream_credentials::delete(transaction, id, expected_updated_at).await,
        ControlPlaneMutation::CreateProxy(input) => {
            proxy_insert(transaction, Uuid::new_v4(), input).await
        }
        ControlPlaneMutation::UpdateProxy {
            id,
            input,
            expected_updated_at,
        } => proxy_update(transaction, id, input, expected_updated_at).await,
        ControlPlaneMutation::DeleteProxy {
            id,
            expected_updated_at,
        } => proxy_delete(transaction, id, expected_updated_at).await,
        ControlPlaneMutation::CreateConfigTemplate(input) => {
            config_template_insert(transaction, Uuid::new_v4(), input, true, None).await
        }
        ControlPlaneMutation::UpdateConfigTemplate {
            id,
            input,
            expected_updated_at,
        } => config_template_insert(transaction, id, input, false, Some(expected_updated_at)).await,
        ControlPlaneMutation::UpdateSystemSettings {
            input,
            expected_updated_at,
        } => system_settings_update(transaction, input, expected_updated_at).await,
    }
}

async fn ensure_api_key_owner_exists(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<(), RepositoryError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users \
         WHERE id=? AND deleted_at IS NULL AND is_system=0)",
    )
    .bind(SqliteUuid(id))
    .fetch_one(&mut **transaction)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

async fn validate_policy_targets(
    transaction: &mut Transaction<'static, Sqlite>,
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
) -> Result<(), RepositoryError> {
    validate_target_lists(allowed_group_ids, allowed_channel_ids, false)?;
    let groups = uuid_array_text(allowed_group_ids)?;
    let group_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM channel_groups AS g \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS selected \
                       WHERE selected.value=g.id) \
           AND g.deleted_at IS NULL",
    )
    .bind(&groups)
    .fetch_one(&mut **transaction)
    .await?;
    let channels = uuid_array_text(allowed_channel_ids)?;
    let channel_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM channels AS channel \
         JOIN channel_groups AS channel_group \
           ON channel_group.id=channel.channel_group_id AND channel_group.deleted_at IS NULL \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS selected \
                       WHERE selected.value=channel.id) \
           AND channel.deleted_at IS NULL",
    )
    .bind(&channels)
    .fetch_one(&mut **transaction)
    .await?;
    if group_count != allowed_group_ids.len() as i64
        || channel_count != allowed_channel_ids.len() as i64
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

async fn channel_is_provider_managed(
    transaction: &mut Transaction<'static, Sqlite>,
    channel_id: Uuid,
) -> Result<bool, RepositoryError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT g.connector_kind <> 'openai_compatible' \
         FROM channels AS c JOIN channel_groups AS g ON g.id=c.channel_group_id \
         WHERE c.id=? AND c.deleted_at IS NULL AND g.deleted_at IS NULL",
    )
    .bind(SqliteUuid(channel_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)
}

fn validate_user_status_transition(
    current_status: &str,
    next_status: &str,
    can_reissue_invitation: bool,
) -> Result<(), RepositoryError> {
    if !matches!(next_status, "active" | "suspended" | "disabled") {
        return Err(RepositoryError::Validation);
    }
    if current_status != next_status
        && !matches!(current_status, "active" | "suspended" | "disabled")
    {
        return Err(RepositoryError::Validation);
    }
    if next_status == "active" && current_status != "active" && can_reissue_invitation {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn default_user_group_id(role: &str) -> Result<Uuid, RepositoryError> {
    match role {
        "user" => Ok(DEFAULT_USER_GROUP_ID),
        "admin" => Ok(DEFAULT_ADMIN_GROUP_ID),
        _ => Err(RepositoryError::Validation),
    }
}

async fn user_group_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: UserGroupInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input.name.trim().is_empty()
        || input.name.len() > 100
        || input
            .description
            .as_ref()
            .is_some_and(|description| description.len() > 500)
    {
        return Err(RepositoryError::Validation);
    }
    let before = if create {
        json!({})
    } else {
        user_group_audit(transaction, id).await?
    };
    if !create && !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    if let Some(policy_id) = input.default_api_key_policy_id {
        let current_policy_id = before["default_api_key_policy_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok());
        if create || current_policy_id != Some(policy_id) {
            ensure_enabled_policy(transaction, policy_id).await?;
        }
    }
    let updated_at = if create {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO user_groups \
             (id,name,description,default_api_key_policy_id,filter_fast_mode) \
             VALUES (?,?,?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(&input.name)
        .bind(&input.description)
        .bind(input.default_api_key_policy_id.map(SqliteUuid))
        .bind(input.filter_fast_mode)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE user_groups SET name=?,description=?,default_api_key_policy_id=?, \
                    filter_fast_mode=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
        )
        .bind(&input.name)
        .bind(&input.description)
        .bind(input.default_api_key_policy_id.map(SqliteUuid))
        .bind(input.filter_fast_mode)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at.expect("PUT version")))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    replace_user_group_codex_quota_visibility(
        transaction,
        id,
        &input.visible_codex_quota_group_ids,
    )
    .await?;
    Ok(MutationResult {
        id,
        object_type: "user_group",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: user_group_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn replace_user_group_codex_quota_visibility(
    transaction: &mut Transaction<'static, Sqlite>,
    user_group_id: Uuid,
    channel_group_ids: &[Uuid],
) -> Result<(), RepositoryError> {
    let unique_ids = channel_group_ids.iter().copied().collect::<HashSet<_>>();
    if unique_ids.len() != channel_group_ids.len() {
        return Err(RepositoryError::Validation);
    }
    if !channel_group_ids.is_empty() {
        let selected = uuid_array_text(channel_group_ids)?;
        let valid_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM channel_groups AS g \
             WHERE EXISTS (SELECT 1 FROM json_each(?) AS allowed \
                           WHERE allowed.value=g.id) \
               AND g.deleted_at IS NULL AND g.connector_kind='codex_oauth' \
               AND g.api_format='open_ai_responses'",
        )
        .bind(&selected)
        .fetch_one(&mut **transaction)
        .await?;
        if valid_count != channel_group_ids.len() as i64 {
            return Err(RepositoryError::Validation);
        }
    }
    sqlx::query("DELETE FROM user_group_codex_quota_visibility WHERE user_group_id=?")
        .bind(SqliteUuid(user_group_id))
        .execute(&mut **transaction)
        .await?;
    for channel_group_id in channel_group_ids {
        sqlx::query(
            "INSERT INTO user_group_codex_quota_visibility (user_group_id,channel_group_id) \
             VALUES (?,?)",
        )
        .bind(SqliteUuid(user_group_id))
        .bind(SqliteUuid(*channel_group_id))
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn user_group_soft_delete(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    deleted_by: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = user_group_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    if !before["system_role"].is_null() {
        return Err(RepositoryError::ProtectedUserGroup);
    }

    let reassigned_users = sqlx::query(
        "UPDATE users SET user_group_id=CASE role WHEN 'admin' THEN ? ELSE ? END, \
                updated_at=ag_now() \
         WHERE user_group_id=? AND deleted_at IS NULL AND is_system=0",
    )
    .bind(SqliteUuid(DEFAULT_ADMIN_GROUP_ID))
    .bind(SqliteUuid(DEFAULT_USER_GROUP_ID))
    .bind(SqliteUuid(id))
    .execute(&mut **transaction)
    .await?;
    let disabled_codes = sqlx::query(
        "UPDATE registration_invitation_codes SET enabled=0,updated_at=ag_now() \
         WHERE user_group_id=? AND enabled=1",
    )
    .bind(SqliteUuid(id))
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM user_group_codex_quota_visibility WHERE user_group_id=?")
        .bind(SqliteUuid(id))
        .execute(&mut **transaction)
        .await?;
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE user_groups SET deleted_at=ag_now(),deleted_by=?,updated_at=ag_now() \
         WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(SqliteUuid(deleted_by))
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "user_group",
        action: "delete",
        before_redacted: before,
        after_redacted: user_group_audit(transaction, id).await?,
        created_secret: None,
        reason: Some(format!(
            "{} users reassigned; {} registration invitation codes disabled",
            reassigned_users.rows_affected(),
            disabled_codes.rows_affected()
        )),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn resolve_user_group_id(
    transaction: &mut Transaction<'static, Sqlite>,
    requested: Option<Uuid>,
    role: &str,
) -> Result<Uuid, RepositoryError> {
    let id = requested.unwrap_or(default_user_group_id(role)?);
    ensure_user_group_exists(transaction, id).await?;
    Ok(id)
}

async fn user_create(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: UserInput,
) -> Result<MutationResult, RepositoryError> {
    let user_group_id =
        resolve_user_group_id(transaction, input.user_group_id, &input.role).await?;
    if let Some(policy_id) = input.default_api_key_policy_id {
        ensure_enabled_policy(transaction, policy_id).await?;
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO users \
         (id,email,display_name,role,status,balance_amount,user_group_id,default_api_key_policy_id) \
         VALUES (?,?,?,?,?,?,?,?) RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(&input.email)
    .bind(&input.display_name)
    .bind(&input.role)
    .bind(&input.status)
    .bind(amount_24_8(input.balance_amount)?)
    .bind(SqliteUuid(user_group_id))
    .bind(input.default_api_key_policy_id.map(SqliteUuid))
    .fetch_one(&mut **transaction)
    .await?;
    Ok(MutationResult {
        id,
        object_type: "user",
        action: "create",
        before_redacted: json!({}),
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn user_update(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: UserUpdateInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.is_empty() {
        return Err(RepositoryError::Validation);
    }
    let before = user_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    validate_user_update(transaction, &before, &input).await?;

    let email_changed = input
        .email
        .as_ref()
        .is_some_and(|email| before["email"].as_str() != email.as_deref());
    let role_changed = input
        .role
        .as_deref()
        .is_some_and(|role| before["role"].as_str() != Some(role));
    let status_changed = input
        .status
        .as_deref()
        .is_some_and(|status| before["status"].as_str() != Some(status));
    let invalidates_sessions = email_changed || role_changed || status_changed;
    let resolved_group_id = if let Some(group_id) = input.user_group_id {
        Some(group_id)
    } else if role_changed && before["user_group_system_role"].as_str() == before["role"].as_str() {
        Some(default_user_group_id(
            input.role.as_deref().ok_or(RepositoryError::Validation)?,
        )?)
    } else {
        None
    };
    if let Some(group_id) = resolved_group_id {
        ensure_user_group_exists(transaction, group_id).await?;
    }

    let UserUpdateInput {
        display_name,
        email,
        role,
        status,
        balance_amount,
        user_group_id: _,
        default_api_key_policy_id,
        websocket_enabled,
    } = input;
    let email_present = email.is_some();
    let email = email.flatten();
    let policy_present = default_api_key_policy_id.is_some();
    let default_api_key_policy_id = default_api_key_policy_id.flatten();
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE users SET \
             email=CASE WHEN ? THEN ? ELSE email END, \
             display_name=COALESCE(?,display_name), \
             role=COALESCE(?,role), \
             status=COALESCE(?,status), \
             balance_amount=COALESCE(?,balance_amount), \
             user_group_id=COALESCE(?,user_group_id), \
             default_api_key_policy_id=CASE WHEN ? THEN ? ELSE default_api_key_policy_id END, \
             websocket_enabled=COALESCE(?,websocket_enabled), \
             auth_version=auth_version+CASE WHEN ? THEN 1 ELSE 0 END, \
             updated_at=ag_now() \
         WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(email_present)
    .bind(email)
    .bind(display_name)
    .bind(role)
    .bind(status)
    .bind(balance_amount.map(amount_24_8).transpose()?)
    .bind(resolved_group_id.map(SqliteUuid))
    .bind(policy_present)
    .bind(default_api_key_policy_id.map(SqliteUuid))
    .bind(websocket_enabled)
    .bind(invalidates_sessions)
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    if invalidates_sessions {
        revoke_live_sessions(transaction, id).await?;
    }
    Ok(MutationResult {
        id,
        object_type: "user",
        action: "update",
        before_redacted: before,
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn validate_user_update(
    transaction: &mut Transaction<'static, Sqlite>,
    before: &Value,
    input: &UserUpdateInput,
) -> Result<(), RepositoryError> {
    if input
        .display_name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty() || name.len() > 200)
        || input.email.as_ref().is_some_and(|email| {
            email.as_ref().is_some_and(|email| {
                let email = email.trim();
                email.is_empty()
                    || email.len() > 320
                    || email.bytes().any(|byte| byte.is_ascii_whitespace())
                    || !email.contains('@')
            })
        })
        || input
            .role
            .as_deref()
            .is_some_and(|role| !matches!(role, "user" | "admin"))
    {
        return Err(RepositoryError::Validation);
    }
    if let Some(next_status) = input.status.as_deref() {
        let current_status = before["status"]
            .as_str()
            .ok_or(RepositoryError::Validation)?;
        validate_user_status_transition(
            current_status,
            next_status,
            before["can_reissue_invitation"].as_bool() == Some(true),
        )?;
    }
    if let Some(Some(policy_id)) = input.default_api_key_policy_id {
        let current_policy_id = before["default_api_key_policy_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok());
        if current_policy_id != Some(policy_id) {
            ensure_enabled_policy(transaction, policy_id).await?;
        }
    }
    if let Some(group_id) = input.user_group_id {
        ensure_user_group_exists(transaction, group_id).await?;
    }
    Ok(())
}

async fn user_soft_delete(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    deleted_by: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if id == deleted_by {
        return Err(RepositoryError::CannotDeleteSelf);
    }
    let before = user_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    if before["role"].as_str() == Some("admin") && before["status"].as_str() == Some("active") {
        let remaining = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM users \
             WHERE role='admin' AND status='active' AND is_system=0 \
               AND deleted_at IS NULL AND id<>?",
        )
        .bind(SqliteUuid(id))
        .fetch_one(&mut **transaction)
        .await?;
        if remaining == 0 {
            return Err(RepositoryError::LastAdministrator);
        }
    }
    let deleted_name = format!("Deleted user {}", id.simple());
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE users SET \
             email=NULL,display_name=?,role='user',status='disabled',password_hash=NULL, \
             password_change_required=0,temporary_password_issued_at=NULL, \
             temporary_password_expires_at=NULL,auth_version=auth_version+1, \
             user_group_id=?,default_api_key_policy_id=NULL,deleted_at=ag_now(),deleted_by=?, \
             updated_at=ag_now() \
         WHERE id=? AND updated_at=? AND deleted_at IS NULL AND is_system=0 \
         RETURNING updated_at",
    )
    .bind(&deleted_name)
    .bind(SqliteUuid(DEFAULT_USER_GROUP_ID))
    .bind(SqliteUuid(deleted_by))
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    revoke_live_sessions(transaction, id).await?;
    sqlx::query(
        "UPDATE user_invitations SET revoked_at=ag_now() \
         WHERE user_id=? AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(SqliteUuid(id))
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE api_keys SET status='revoked',secret_value='deleted-api-key-' || id, \
                deleted_at=ag_now(),deleted_by=?,updated_at=ag_now() \
         WHERE user_id=? AND deleted_at IS NULL AND is_system=0",
    )
    .bind(SqliteUuid(deleted_by))
    .bind(SqliteUuid(id))
    .execute(&mut **transaction)
    .await?;
    Ok(MutationResult {
        id,
        object_type: "user",
        action: "delete",
        before_redacted: before,
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: Some("user anonymized and API keys deleted".into()),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn api_key_policy_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: ApiKeyPolicyInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input.name.trim().is_empty() {
        return Err(RepositoryError::Validation);
    }
    validate_policy_targets(
        transaction,
        &input.allowed_group_ids,
        &input.allowed_channel_ids,
    )
    .await?;
    let before = if create {
        json!({})
    } else {
        api_key_policy_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids,enabled) \
             VALUES (?,?,?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(&input.name)
        .bind(value_to_text(&json!(input.allowed_group_ids))?)
        .bind(value_to_text(&json!(input.allowed_channel_ids))?)
        .bind(input.enabled)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE api_key_policies \
             SET name=?,allowed_group_ids=?,allowed_channel_ids=?,enabled=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? RETURNING updated_at",
        )
        .bind(&input.name)
        .bind(value_to_text(&json!(input.allowed_group_ids))?)
        .bind(value_to_text(&json!(input.allowed_channel_ids))?)
        .bind(input.enabled)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at.expect("PUT version")))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "api_key_policy",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: api_key_policy_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn group_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: ChannelGroupInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input.name.trim().is_empty()
        || ApiFormat::parse(&input.api_format).is_none()
        || !matches!(
            input.connector_kind.as_str(),
            "openai_compatible" | "codex_oauth"
        )
        || input
            .request_compression
            .as_deref()
            .is_some_and(|value| RequestCompression::parse(value).is_none())
        || (create
            && input.connector_kind == "codex_oauth"
            && input.api_format != "open_ai_responses")
    {
        return Err(RepositoryError::Validation);
    }
    let before = if create {
        json!({})
    } else {
        group_audit(transaction, id).await?
    };
    if !create && !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let sharing_only = input
        .sharing_only
        .unwrap_or_else(|| before["sharing_only"].as_bool().unwrap_or(false));
    if sharing_only && input.connector_kind != "codex_oauth" {
        return Err(RepositoryError::Validation);
    }
    let request_compression = input.request_compression.as_deref().unwrap_or_else(|| {
        if create {
            "default"
        } else {
            before["request_compression"]
                .as_str()
                .expect("persisted channel group request compression is a string")
        }
    });
    if RequestCompression::parse(request_compression)
        .expect("validated channel group request compression")
        .is_encoded()
        && input.api_format != "open_ai_responses"
    {
        return Err(RepositoryError::Validation);
    }
    if !create && before["connector_kind"].as_str() != Some(input.connector_kind.as_str()) {
        return Err(RepositoryError::Validation);
    }
    if !create
        && input.connector_kind == "codex_oauth"
        && before["api_format"].as_str() != Some(input.api_format.as_str())
    {
        return Err(RepositoryError::Validation);
    }
    // A new Codex Responses group must reference an existing connector pool in
    // the same transaction; SQLite cannot assign one in a BEFORE trigger.
    let connector_pool_id = if create && input.connector_kind == "codex_oauth" {
        let pool_id = Uuid::new_v4();
        sqlx::query("INSERT INTO connector_pools (id,connector_kind) VALUES (?,'codex_oauth')")
            .bind(SqliteUuid(pool_id))
            .execute(&mut **transaction)
            .await?;
        Some(pool_id)
    } else {
        before["connector_pool_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
    };
    let updated_at = if create {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO channel_groups \
             (id,name,api_format,connector_kind,connector_pool_id,request_compression,enabled, \
              status_statistics_enabled,sharing_only) \
             VALUES (?,?,?,?,?,?,?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(&input.name)
        .bind(&input.api_format)
        .bind(&input.connector_kind)
        .bind(connector_pool_id.map(SqliteUuid))
        .bind(request_compression)
        .bind(input.enabled)
        .bind(input.status_statistics_enabled.unwrap_or(false))
        .bind(sharing_only)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE channel_groups SET name=?,api_format=?,connector_kind=?, \
                    request_compression=COALESCE(?,request_compression),enabled=?, \
                    status_statistics_enabled=COALESCE(?,status_statistics_enabled), \
                    sharing_only=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
        )
        .bind(&input.name)
        .bind(&input.api_format)
        .bind(&input.connector_kind)
        .bind(&input.request_compression)
        .bind(input.enabled)
        .bind(input.status_statistics_enabled)
        .bind(sharing_only)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at.expect("PUT version")))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "channel_group",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: group_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn model_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: ModelInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input
        .source_payload
        .as_ref()
        .is_some_and(|payload| payload.as_object().is_none())
    {
        return Err(RepositoryError::Validation);
    }
    if input.advanced_billing.as_ref().is_some_and(|billing| {
        crate::domain::CompiledAdvancedBilling::compile(billing.clone()).is_err()
    }) {
        return Err(RepositoryError::Validation);
    }
    let advanced_billing_present = input.advanced_billing.is_some();
    let advanced_billing =
        serde_json::to_value(input.advanced_billing.unwrap_or_default()).expect("serializes");
    let source_payload_present = input.source_payload.is_some();
    let source_payload = input.source_payload.unwrap_or_else(|| json!({}));
    let before = if create {
        json!({})
    } else {
        model_audit(transaction, id).await?
    };
    if !create && !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = if create {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO models \
             (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens, \
              input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price, \
              price_effective_at,advanced_billing,source_payload) \
             VALUES (?,?,?,?,?,'USD',?,?,?,?,?,?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(&input.source_model_id)
        .bind(&input.display_name)
        .bind(&input.provider_name)
        .bind(input.enabled)
        .bind(input.price_unit_tokens)
        .bind(amount_24_12(input.input_unit_price)?)
        .bind(amount_24_12(input.cached_input_unit_price)?)
        .bind(amount_24_12(input.cache_write_unit_price)?)
        .bind(amount_24_12(input.output_unit_price)?)
        .bind(SqliteTimestamp(input.price_effective_at))
        .bind(value_to_text(&advanced_billing)?)
        .bind(value_to_text(&source_payload)?)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE models SET source_model_id=?,display_name=?,provider_name=?,enabled=?, \
                    currency='USD',price_unit_tokens=?,input_unit_price=?, \
                    cached_input_unit_price=?,cache_write_unit_price=?,output_unit_price=?, \
                    price_effective_at=?, \
                    advanced_billing=CASE WHEN ? THEN ? ELSE advanced_billing END, \
                    source_payload=CASE WHEN ? THEN ? ELSE source_payload END, \
                    updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
        )
        .bind(&input.source_model_id)
        .bind(&input.display_name)
        .bind(&input.provider_name)
        .bind(input.enabled)
        .bind(input.price_unit_tokens)
        .bind(amount_24_12(input.input_unit_price)?)
        .bind(amount_24_12(input.cached_input_unit_price)?)
        .bind(amount_24_12(input.cache_write_unit_price)?)
        .bind(amount_24_12(input.output_unit_price)?)
        .bind(SqliteTimestamp(input.price_effective_at))
        .bind(advanced_billing_present)
        .bind(value_to_text(&advanced_billing)?)
        .bind(source_payload_present)
        .bind(value_to_text(&source_payload)?)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at.expect("PUT version")))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "model",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: model_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn model_soft_delete(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    deleted_by: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = model_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    let disabled_protocol_rules = sqlx::query(
        "UPDATE model_rules SET enabled=0,updated_at=ag_now() \
         WHERE enabled=1 AND model_routing_profile_id IN ( \
             SELECT id FROM model_routing_profiles WHERE model_id=?)",
    )
    .bind(SqliteUuid(id))
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    let cleared_scheduled_tests = sqlx::query(
        "UPDATE channels SET test_model=NULL,test_pricing_model_id=NULL,updated_at=ag_now() \
         WHERE test_pricing_model_id=?",
    )
    .bind(SqliteUuid(id))
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE models SET enabled=0,deleted_at=ag_now(),deleted_by=?,updated_at=ag_now() \
         WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(SqliteUuid(deleted_by))
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "model",
        action: "delete",
        before_redacted: before,
        after_redacted: model_audit(transaction, id).await?,
        created_secret: None,
        reason: Some(format!(
            "{disabled_protocol_rules} protocol rules disabled; \
             {cleared_scheduled_tests} scheduled test references cleared"
        )),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn import_model(
    transaction: &mut Transaction<'static, Sqlite>,
    input: crate::persistence::SyncedModelInput,
    synced_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.source_payload.as_object().is_none() {
        return Err(RepositoryError::Validation);
    }
    let id = Uuid::new_v4();
    let advanced_billing =
        serde_json::to_value(&input.advanced_billing).expect("advanced billing serializes");
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO models \
         (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens, \
          input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price, \
          price_effective_at,advanced_billing,source_payload,last_synced_at) \
         VALUES (?,?,?,?,1,'USD',1000000,?,?,?,?,?,?,?,?) RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(&input.source_model_id)
    .bind(&input.display_name)
    .bind(&input.provider_name)
    .bind(amount_24_12(input.input_unit_price)?)
    .bind(amount_24_12(input.cached_input_unit_price)?)
    .bind(amount_24_12(input.cache_write_unit_price)?)
    .bind(amount_24_12(input.output_unit_price)?)
    .bind(SqliteTimestamp(synced_at))
    .bind(value_to_text(&advanced_billing)?)
    .bind(value_to_text(&input.source_payload)?)
    .bind(SqliteTimestamp(synced_at))
    .fetch_one(&mut **transaction)
    .await?;
    Ok(MutationResult {
        id,
        object_type: "model",
        action: "import",
        before_redacted: json!({}),
        after_redacted: model_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

/// Refreshes catalog-owned price facts, long-context tiers, and any available
/// request multipliers for a local source model. Display name, provider label,
/// enabled state, and unmatched local request-multiplier rules remain
/// administrator-managed.
async fn sync_model_price(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: crate::persistence::SyncedModelInput,
    synced_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.source_payload.as_object().is_none() {
        return Err(RepositoryError::Validation);
    }
    let current_advanced_billing: String = sqlx::query_scalar(
        "SELECT advanced_billing FROM models \
         WHERE id=? AND source_model_id=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(id))
    .bind(&input.source_model_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    let before = model_audit(transaction, id).await?;
    let advanced_billing = merge_synced_advanced_billing(
        parse_json(&current_advanced_billing)?,
        input.advanced_billing,
    )?;
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE models SET currency='USD',price_unit_tokens=1000000,input_unit_price=?, \
                cached_input_unit_price=?,cache_write_unit_price=?,output_unit_price=?, \
                price_effective_at=?,advanced_billing=?,source_payload=?,last_synced_at=?, \
                updated_at=ag_now() \
         WHERE id=? AND source_model_id=? AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(amount_24_12(input.input_unit_price)?)
    .bind(amount_24_12(input.cached_input_unit_price)?)
    .bind(amount_24_12(input.cache_write_unit_price)?)
    .bind(amount_24_12(input.output_unit_price)?)
    .bind(SqliteTimestamp(synced_at))
    .bind(value_to_text(&advanced_billing)?)
    .bind(value_to_text(&input.source_payload)?)
    .bind(SqliteTimestamp(synced_at))
    .bind(SqliteUuid(id))
    .bind(&input.source_model_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "model",
        action: "price_sync",
        before_redacted: before,
        after_redacted: model_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

fn merge_synced_advanced_billing(
    current: Value,
    synced: crate::domain::AdvancedBilling,
) -> Result<Value, RepositoryError> {
    let mut merged = serde_json::from_value::<crate::domain::AdvancedBilling>(current)
        .map_err(|_| RepositoryError::Validation)?;
    merged.long_context_tiers = synced.long_context_tiers;
    if !synced.request_multipliers.is_empty() {
        merged.request_multipliers.retain(|current| {
            !synced.request_multipliers.iter().any(|catalog| {
                current.json_pointer == catalog.json_pointer && current.value == catalog.value
            })
        });
        merged
            .request_multipliers
            .extend(synced.request_multipliers);
    }
    crate::domain::CompiledAdvancedBilling::compile(merged.clone())
        .map_err(|_| RepositoryError::Validation)?;
    serde_json::to_value(merged).map_err(|_| RepositoryError::Validation)
}

/// Applies explicitly selected catalog entries. Existing source-model IDs
/// receive a price refresh; absent IDs are imported as new local models.
async fn apply_catalog_models(
    transaction: &mut Transaction<'static, Sqlite>,
    inputs: Vec<crate::persistence::SyncedModelInput>,
) -> Result<Vec<MutationResult>, RepositoryError> {
    let synced_at = Utc::now();
    let mut results = Vec::with_capacity(inputs.len());
    for input in inputs {
        let existing_id = sqlx::query_scalar::<_, SqliteUuid>(
            "SELECT id FROM models WHERE source_model_id=? AND deleted_at IS NULL",
        )
        .bind(&input.source_model_id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(|value| value.0);
        results.push(match existing_id {
            Some(id) => sync_model_price(transaction, id, input, synced_at).await?,
            None => import_model(transaction, input, synced_at).await?,
        });
    }
    Ok(results)
}

async fn channel_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: impl Into<ChannelMutationInput>,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let input = input.into();
    if !create && channel_is_provider_managed(transaction, id).await? {
        return Err(RepositoryError::Validation);
    }
    let connector_kind = sqlx::query_scalar::<_, String>(
        "SELECT connector_kind FROM channel_groups WHERE id=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(input.channel_group_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Validation)?;
    if connector_kind != "openai_compatible" {
        return Err(RepositoryError::Validation);
    }
    super::upstream_credentials::validate_binding(
        transaction,
        input.credential_id,
        &input.base_url,
    )
    .await?;
    if input
        .override_document
        .as_ref()
        .is_some_and(|document| document.as_object().is_none())
    {
        return Err(RepositoryError::Validation);
    }
    if ApiFormat::parse(&input.api_format).is_none() {
        return Err(RepositoryError::Validation);
    }
    if input
        .billing_multiplier
        .is_some_and(|multiplier| multiplier.is_sign_negative())
    {
        return Err(RepositoryError::Validation);
    }
    if (input.supports_websocket || input.supports_standalone_web_search)
        && input.api_format != "open_ai_responses"
    {
        return Err(RepositoryError::Validation);
    }
    if input.api_format == "open_ai_images"
        && (input.test_model.is_some() || input.test_pricing_model_id.is_some())
    {
        return Err(RepositoryError::Validation);
    }
    if input.test_model.is_some() != input.test_pricing_model_id.is_some() {
        return Err(RepositoryError::Validation);
    }
    if input.test_model.as_ref().is_some_and(|model| {
        !input
            .available_models
            .iter()
            .any(|available| available == model)
    }) {
        return Err(RepositoryError::Validation);
    }
    if let Some(test_pricing_model_id) = input.test_pricing_model_id {
        let configured = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM models WHERE id=? AND deleted_at IS NULL)",
        )
        .bind(SqliteUuid(test_pricing_model_id))
        .fetch_one(&mut **transaction)
        .await?;
        if !configured {
            return Err(RepositoryError::Validation);
        }
    }
    let override_document_present = input.override_document.is_some();
    let override_document = input.override_document.unwrap_or_else(|| json!({}));
    let before = if create {
        json!({})
    } else {
        channel_audit(transaction, id).await?
    };
    if !create && !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = if create {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO channels \
             (id,channel_group_id,api_format,name,base_url,enabled,billing_multiplier,proxy_id, \
              config_template_id,override_document,connect_timeout_ms,response_header_timeout_ms, \
              stream_idle_timeout_ms,upstream_auth_kind,credential_id, \
              available_models,test_model,test_pricing_model_id,auto_disable_allowed, \
              supports_websocket,supports_standalone_web_search) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,'none',?,?,?,?,?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(input.channel_group_id))
        .bind(&input.api_format)
        .bind(&input.name)
        .bind(&input.base_url)
        .bind(input.enabled)
        .bind(amount_24_12(
            input.billing_multiplier.unwrap_or(Decimal::ONE),
        )?)
        .bind(input.proxy_id.map(SqliteUuid))
        .bind(input.config_template_id.map(SqliteUuid))
        .bind(value_to_text(&override_document)?)
        .bind(input.connect_timeout_ms)
        .bind(input.response_header_timeout_ms)
        .bind(input.stream_idle_timeout_ms)
        .bind(input.credential_id.map(SqliteUuid))
        .bind(json_text(&input.available_models)?)
        .bind(&input.test_model)
        .bind(input.test_pricing_model_id.map(SqliteUuid))
        .bind(input.auto_disable_allowed)
        .bind(input.supports_websocket)
        .bind(input.supports_standalone_web_search)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE channels SET channel_group_id=?,api_format=?,name=?,base_url=?,enabled=?, \
                    billing_multiplier=COALESCE(?,billing_multiplier),proxy_id=?,config_template_id=?, \
                    override_document=CASE WHEN ? THEN ? ELSE override_document END, \
                    connect_timeout_ms=?,response_header_timeout_ms=?,stream_idle_timeout_ms=?, \
                    credential_id=?, \
                    available_models=?,test_model=?,test_pricing_model_id=?,auto_disable_allowed=?, \
                    supports_websocket=?,supports_standalone_web_search=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
        )
        .bind(SqliteUuid(input.channel_group_id))
        .bind(&input.api_format)
        .bind(&input.name)
        .bind(&input.base_url)
        .bind(input.enabled)
        .bind(input.billing_multiplier.map(amount_24_12).transpose()?)
        .bind(input.proxy_id.map(SqliteUuid))
        .bind(input.config_template_id.map(SqliteUuid))
        .bind(override_document_present)
        .bind(value_to_text(&override_document)?)
        .bind(input.connect_timeout_ms)
        .bind(input.response_header_timeout_ms)
        .bind(input.stream_idle_timeout_ms)
        .bind(input.credential_id.map(SqliteUuid))
        .bind(json_text(&input.available_models)?)
        .bind(&input.test_model)
        .bind(input.test_pricing_model_id.map(SqliteUuid))
        .bind(input.auto_disable_allowed)
        .bind(input.supports_websocket)
        .bind(input.supports_standalone_web_search)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at.expect("PUT version")))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "channel",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: channel_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn channel_recover(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = channel_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at || before["auto_disabled"].as_bool() != Some(true)
    {
        return Err(RepositoryError::Conflict);
    }
    let reason = "manually recovered by administrator".to_owned();
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE channels SET auto_disabled=0,auto_disabled_reason=NULL,updated_at=ag_now() \
         WHERE id=? AND updated_at=? AND auto_disabled=1 AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "channel",
        action: "manual_recover",
        before_redacted: before,
        after_redacted: channel_audit(transaction, id).await?,
        created_secret: None,
        reason: Some(reason),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn channel_resource_soft_delete(
    transaction: &mut Transaction<'static, Sqlite>,
    target: ChannelDeletionTarget,
    deleted_by: Uuid,
    expected_updated_at: DateTime<Utc>,
    confirmation_token: &str,
) -> Result<MutationResult, RepositoryError> {
    if confirmation_token.is_empty() {
        return Err(RepositoryError::Validation);
    }
    let (before, object_type) = match target {
        ChannelDeletionTarget::Group(id) => (group_audit(transaction, id).await?, "channel_group"),
        ChannelDeletionTarget::Channel(id) => (channel_audit(transaction, id).await?, "channel"),
    };
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    let plan = load_channel_deletion_plan(transaction, target).await?;
    if plan.root_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    if plan.impact.confirmation_token != confirmation_token {
        return Err(RepositoryError::DeletionImpactChanged);
    }

    let deleted_groups = uuid_array_text(&plan.deleted_group_ids)?;
    let deleted_channels = uuid_array_text(&plan.deleted_channel_ids)?;
    let unbound_keys = sqlx::query(
        "UPDATE api_keys SET \
             allowed_group_ids=(SELECT COALESCE(json_group_array(value),'[]') \
                 FROM json_each(api_keys.allowed_group_ids) AS allowed \
                 WHERE NOT EXISTS (SELECT 1 FROM json_each(?) AS target \
                                   WHERE target.value=allowed.value)), \
             allowed_channel_ids=(SELECT COALESCE(json_group_array(value),'[]') \
                 FROM json_each(api_keys.allowed_channel_ids) AS allowed \
                 WHERE NOT EXISTS (SELECT 1 FROM json_each(?) AS target \
                                   WHERE target.value=allowed.value)), \
             updated_at=ag_now() \
         WHERE deleted_at IS NULL \
           AND (EXISTS (SELECT 1 FROM json_each(?) AS target \
                        JOIN json_each(api_keys.allowed_group_ids) AS allowed \
                          ON allowed.value=target.value) \
                OR EXISTS (SELECT 1 FROM json_each(?) AS target \
                           JOIN json_each(api_keys.allowed_channel_ids) AS allowed \
                             ON allowed.value=target.value))",
    )
    .bind(&deleted_groups)
    .bind(&deleted_channels)
    .bind(&deleted_groups)
    .bind(&deleted_channels)
    .execute(&mut **transaction)
    .await?;
    let unbound_policies = sqlx::query(
        "UPDATE api_key_policies SET \
             allowed_group_ids=(SELECT COALESCE(json_group_array(value),'[]') \
                 FROM json_each(api_key_policies.allowed_group_ids) AS allowed \
                 WHERE NOT EXISTS (SELECT 1 FROM json_each(?) AS target \
                                   WHERE target.value=allowed.value)), \
             allowed_channel_ids=(SELECT COALESCE(json_group_array(value),'[]') \
                 FROM json_each(api_key_policies.allowed_channel_ids) AS allowed \
                 WHERE NOT EXISTS (SELECT 1 FROM json_each(?) AS target \
                                   WHERE target.value=allowed.value)), \
             updated_at=ag_now() \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       JOIN json_each(api_key_policies.allowed_group_ids) AS allowed \
                         ON allowed.value=target.value) \
            OR EXISTS (SELECT 1 FROM json_each(?) AS target \
                       JOIN json_each(api_key_policies.allowed_channel_ids) AS allowed \
                         ON allowed.value=target.value)",
    )
    .bind(&deleted_groups)
    .bind(&deleted_channels)
    .bind(&deleted_groups)
    .bind(&deleted_channels)
    .execute(&mut **transaction)
    .await?;
    let visibility_user_group_ids = plan
        .impact
        .quota_visibility_user_groups
        .iter()
        .map(|group| group.id)
        .collect::<Vec<_>>();
    let visibility_user_groups = uuid_array_text(&visibility_user_group_ids)?;
    sqlx::query(
        "UPDATE user_groups SET updated_at=ag_now() \
         WHERE deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=user_groups.id)",
    )
    .bind(&visibility_user_groups)
    .execute(&mut **transaction)
    .await?;
    let removed_visibility = sqlx::query(
        "DELETE FROM user_group_codex_quota_visibility \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=channel_group_id)",
    )
    .bind(&deleted_groups)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM model_rule_routing_candidates \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=model_rule_id) \
           AND EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=channel_id)",
    )
    .bind(&uuid_array_text(&plan.affected_rule_ids)?)
    .bind(&deleted_channels)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM model_rule_routing_tiers \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=model_rule_id) \
           AND NOT EXISTS (SELECT 1 FROM model_rule_routing_candidates AS candidate \
                           WHERE candidate.model_rule_id=model_rule_routing_tiers.model_rule_id \
                             AND candidate.priority=model_rule_routing_tiers.priority)",
    )
    .bind(&uuid_array_text(&plan.affected_rule_ids)?)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE model_rules SET enabled=CASE WHEN EXISTS ( \
                 SELECT 1 FROM model_rule_routing_tiers AS tier \
                 WHERE tier.model_rule_id=model_rules.id) THEN enabled ELSE 0 END, \
             updated_at=ag_now() \
         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=model_rules.id)",
    )
    .bind(&uuid_array_text(&plan.affected_rule_ids)?)
    .execute(&mut **transaction)
    .await?;
    let tombstoned_channels = sqlx::query(
        "UPDATE channels SET \
             enabled=0,auto_disabled=0,auto_disabled_reason=NULL,auto_disable_allowed=0, \
             supports_websocket=0,supports_standalone_web_search=0, \
             base_url='https://deleted.invalid',billing_multiplier=1,proxy_id=NULL, \
             config_template_id=NULL,override_document='{}',connect_timeout_ms=NULL, \
             response_header_timeout_ms=NULL,stream_idle_timeout_ms=NULL, \
             upstream_auth_kind='none',upstream_auth_header_name=NULL,upstream_api_key=NULL,credential_id=NULL, \
             available_models='[]',test_model=NULL,test_pricing_model_id=NULL, \
             deleted_at=ag_now(),deleted_by=?,updated_at=ag_now() \
         WHERE channels.deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM json_each(?) AS target \
                       WHERE target.value=channels.id)",
    )
    .bind(SqliteUuid(deleted_by))
    .bind(&deleted_channels)
    .execute(&mut **transaction)
    .await?;
    if tombstoned_channels.rows_affected() != plan.deleted_channel_ids.len() as u64 {
        return Err(RepositoryError::Conflict);
    }
    let updated_at = match target {
        ChannelDeletionTarget::Group(id) => sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE channel_groups SET enabled=0,status_statistics_enabled=0, \
                    deleted_at=ag_now(),deleted_by=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? AND deleted_at IS NULL RETURNING updated_at",
        )
        .bind(SqliteUuid(deleted_by))
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?,
        ChannelDeletionTarget::Channel(id) => {
            sqlx::query_scalar::<_, SqliteTimestamp>("SELECT updated_at FROM channels WHERE id=?")
                .bind(SqliteUuid(id))
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(RepositoryError::Conflict)?
        }
    };
    let after = match target {
        ChannelDeletionTarget::Group(id) => group_audit(transaction, id).await?,
        ChannelDeletionTarget::Channel(id) => channel_audit(transaction, id).await?,
    };
    Ok(MutationResult {
        id: plan.impact.resource_id,
        object_type,
        action: "delete",
        before_redacted: before,
        after_redacted: after,
        created_secret: None,
        reason: Some(format!(
            "{} channels deleted; {} protocol rules affected; {} API keys and {} policies unbound; {} quota visibility assignments removed",
            plan.deleted_channel_ids.len(),
            plan.affected_rule_ids.len(),
            unbound_keys.rows_affected(),
            unbound_policies.rows_affected(),
            removed_visibility.rows_affected(),
        )),
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn model_routing_profile_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: ModelRuleCreateInput,
) -> Result<MutationResult, RepositoryError> {
    let model_enabled = sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM models WHERE id=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(input.model_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    if !model_enabled {
        return Err(RepositoryError::Validation);
    }
    let already_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM model_routing_profiles WHERE model_id=?)",
    )
    .bind(SqliteUuid(input.model_id))
    .fetch_one(&mut **transaction)
    .await?;
    if already_exists {
        return Err(RepositoryError::Conflict);
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO model_routing_profiles (id,model_id) VALUES (?,?) \
         ON CONFLICT (model_id) DO NOTHING RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(input.model_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "model_rule",
        action: "create",
        before_redacted: json!({}),
        after_redacted: model_routing_profile_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn model_protocol_rule_create(
    transaction: &mut Transaction<'static, Sqlite>,
    model_rule_id: Uuid,
    id: Uuid,
    input: ModelProtocolRuleCreateInput,
) -> Result<MutationResult, RepositoryError> {
    if ApiFormat::parse(&input.api_format).is_none() {
        return Err(RepositoryError::Validation);
    }
    let profile_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS( \
             SELECT 1 FROM model_routing_profiles AS profile \
             JOIN models AS model ON model.id=profile.model_id \
             WHERE profile.id=? AND model.deleted_at IS NULL)",
    )
    .bind(SqliteUuid(model_rule_id))
    .fetch_one(&mut **transaction)
    .await?;
    if !profile_exists {
        return Err(RepositoryError::NotFound);
    }
    let already_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM model_rules \
         WHERE model_routing_profile_id=? AND api_format=?)",
    )
    .bind(SqliteUuid(model_rule_id))
    .bind(&input.api_format)
    .fetch_one(&mut **transaction)
    .await?;
    if already_exists {
        return Err(RepositoryError::Conflict);
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO model_rules \
         (id,model_routing_profile_id,api_format,description,enabled) \
         VALUES (?,?,?,NULL,0) \
         ON CONFLICT (model_routing_profile_id,api_format) DO NOTHING RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(model_rule_id))
    .bind(&input.api_format)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "model_protocol_rule",
        action: "create",
        before_redacted: json!({}),
        after_redacted: rule_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn model_protocol_rule_update(
    transaction: &mut Transaction<'static, Sqlite>,
    model_rule_id: Uuid,
    id: Uuid,
    input: ModelProtocolRuleInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if (input.enabled && input.routing_tiers.is_empty())
        || !valid_model_rule_routing_tiers(&input.routing_tiers)
    {
        return Err(RepositoryError::Validation);
    }
    let current = sqlx::query_as::<_, (String, SqliteTimestamp)>(
        "SELECT rule.api_format,rule.updated_at \
         FROM model_rules AS rule \
         JOIN model_routing_profiles AS profile ON profile.id=rule.model_routing_profile_id \
         JOIN models AS model ON model.id=profile.model_id AND model.deleted_at IS NULL \
         WHERE rule.id=? AND rule.model_routing_profile_id=?",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(model_rule_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    if current.1.0 != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    let api_format = current.0;
    validate_model_rule_routing_references(transaction, &api_format, &input.routing_tiers).await?;
    let before = rule_audit(transaction, id).await?;
    sqlx::query("DELETE FROM model_rule_routing_tiers WHERE model_rule_id=?")
        .bind(SqliteUuid(id))
        .execute(&mut **transaction)
        .await?;
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE model_rules SET description=?,enabled=?,updated_at=ag_now() \
         WHERE id=? AND model_routing_profile_id=? RETURNING updated_at",
    )
    .bind(&input.description)
    .bind(input.enabled)
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(model_rule_id))
    .fetch_one(&mut **transaction)
    .await?;
    insert_model_rule_routing_tiers(transaction, id, &api_format, &input.routing_tiers).await?;
    Ok(MutationResult {
        id,
        object_type: "model_protocol_rule",
        action: "update",
        before_redacted: before,
        after_redacted: rule_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

fn valid_model_rule_routing_tiers(tiers: &[ModelRuleRoutingTier]) -> bool {
    let mut priorities = HashSet::with_capacity(tiers.len());
    for tier in tiers {
        if tier.priority < 0
            || !priorities.insert(tier.priority)
            || !matches!(
                tier.selection_strategy.as_str(),
                "weighted_random" | "weighted_round_robin"
            )
            || tier.candidates.is_empty()
        {
            return false;
        }
        let mut candidates = HashSet::with_capacity(tier.candidates.len());
        for candidate in &tier.candidates {
            if candidate.weight <= 0
                || candidate.upstream_model.trim().is_empty()
                || candidate.upstream_model.chars().count() > 300
                || !candidates.insert((candidate.channel_id, candidate.upstream_model.as_str()))
            {
                return false;
            }
        }
    }
    true
}

async fn validate_model_rule_routing_references(
    transaction: &mut Transaction<'static, Sqlite>,
    api_format: &str,
    tiers: &[ModelRuleRoutingTier],
) -> Result<(), RepositoryError> {
    let candidates = tiers
        .iter()
        .flat_map(|tier| &tier.candidates)
        .map(|candidate| (candidate.channel_id, candidate.upstream_model.as_str()))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(());
    }
    let mut matching = 0_i64;
    for (channel_id, upstream_model) in &candidates {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS( \
                 SELECT 1 FROM channels AS channel \
                 WHERE channel.id=? AND channel.api_format=? AND channel.deleted_at IS NULL \
                   AND EXISTS (SELECT 1 FROM json_each(channel.available_models) AS available \
                               WHERE available.value=?))",
        )
        .bind(SqliteUuid(*channel_id))
        .bind(api_format)
        .bind(upstream_model)
        .fetch_one(&mut **transaction)
        .await?;
        if exists {
            matching += 1;
        }
    }
    if usize::try_from(matching).ok() != Some(candidates.len()) {
        return Err(RepositoryError::RoutingDependencyInvalid);
    }
    Ok(())
}

async fn insert_model_rule_routing_tiers(
    transaction: &mut Transaction<'static, Sqlite>,
    model_rule_id: Uuid,
    api_format: &str,
    tiers: &[ModelRuleRoutingTier],
) -> Result<(), RepositoryError> {
    // The rule's shape assertion is a DEFERRABLE INITIALLY DEFERRED foreign
    // key, so a legal delete-and-reinsert of tiers is checked only at commit.
    for tier in tiers {
        sqlx::query(
            "INSERT INTO model_rule_routing_tiers \
             (model_rule_id,api_format,priority,selection_strategy) VALUES (?,?,?,?)",
        )
        .bind(SqliteUuid(model_rule_id))
        .bind(api_format)
        .bind(tier.priority)
        .bind(&tier.selection_strategy)
        .execute(&mut **transaction)
        .await?;
        for candidate in &tier.candidates {
            sqlx::query(
                "INSERT INTO model_rule_routing_candidates \
                 (model_rule_id,api_format,priority,channel_id,upstream_model,weight) \
                 VALUES (?,?,?,?,?,?)",
            )
            .bind(SqliteUuid(model_rule_id))
            .bind(api_format)
            .bind(tier.priority)
            .bind(SqliteUuid(candidate.channel_id))
            .bind(&candidate.upstream_model)
            .bind(candidate.weight)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(())
}

async fn proxy_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: ProxyCreateInput,
) -> Result<MutationResult, RepositoryError> {
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO proxies (id,name,proxy_url,username,password,no_proxy_hosts,enabled) \
         VALUES (?,?,?,?,?,?,?) RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(&input.name)
    .bind(&input.proxy_url)
    .bind(&input.username)
    .bind(&input.password)
    .bind(json_text(&input.no_proxy_hosts)?)
    .bind(input.enabled)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(MutationResult {
        id,
        object_type: "proxy",
        action: "create",
        before_redacted: json!({}),
        after_redacted: proxy_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn proxy_update(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: ProxyInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = proxy_audit(transaction, id).await?;
    let username_present = input.username.is_some();
    let password_present = input.password.is_some();
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE proxies SET name=?,proxy_url=?, \
                username=CASE WHEN ? THEN ? ELSE username END, \
                password=CASE WHEN ? THEN ? ELSE password END, \
                no_proxy_hosts=?,enabled=?,updated_at=ag_now() \
         WHERE id=? AND updated_at=? RETURNING updated_at",
    )
    .bind(&input.name)
    .bind(&input.proxy_url)
    .bind(username_present)
    .bind(input.username.flatten())
    .bind(password_present)
    .bind(input.password.flatten())
    .bind(json_text(&input.no_proxy_hosts)?)
    .bind(input.enabled)
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "proxy",
        action: "update",
        before_redacted: before,
        after_redacted: proxy_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn proxy_delete(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = proxy_audit(transaction, id).await?;
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    let in_use = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM channels WHERE proxy_id=? AND deleted_at IS NULL) \
             OR EXISTS(SELECT 1 FROM codex_oauth_flows WHERE proxy_id=?)",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(id))
    .fetch_one(&mut **transaction)
    .await?;
    if in_use {
        return Err(RepositoryError::ProxyInUse);
    }
    let deleted = sqlx::query("DELETE FROM proxies WHERE id=? AND updated_at=?")
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at))
        .execute(&mut **transaction)
        .await?;
    if deleted.rows_affected() != 1 {
        return Err(RepositoryError::Conflict);
    }
    Ok(MutationResult {
        id,
        object_type: "proxy",
        action: "delete",
        before_redacted: before,
        after_redacted: json!({}),
        created_secret: None,
        reason: None,
        updated_at: expected_updated_at,
        correlation_id: None,
    })
}

async fn config_template_insert(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    input: impl Into<ConfigTemplateMutationInput>,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let input = input.into();
    if create && input.document.is_none() {
        return Err(RepositoryError::Validation);
    }
    let document_present = input.document.is_some();
    let document = input.document.unwrap_or_else(|| json!({}));
    let before = if create {
        json!({})
    } else {
        config_template_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO config_templates (id,name,description,document,enabled) \
             VALUES (?,?,?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(&input.name)
        .bind(&input.description)
        .bind(value_to_text(&document)?)
        .bind(input.enabled)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE config_templates SET name=?,description=?, \
                    document=CASE WHEN ? THEN ? ELSE document END,enabled=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? RETURNING updated_at",
        )
        .bind(&input.name)
        .bind(&input.description)
        .bind(document_present)
        .bind(value_to_text(&document)?)
        .bind(input.enabled)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at.expect("PUT version")))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "config_template",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: config_template_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn system_settings_update(
    transaction: &mut Transaction<'static, Sqlite>,
    input: SystemSettingsInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    validate_system_settings_input(&input)?;
    let value = serde_json::to_value(&input).expect("system settings serialize");
    let before = system_settings_audit(transaction).await?;
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE system_settings SET value=?,updated_at=ag_now() \
         WHERE setting_key=? AND updated_at=? RETURNING updated_at",
    )
    .bind(value_to_text(&value)?)
    .bind(FORWARDING_SETTINGS_KEY)
    .bind(SqliteTimestamp(expected_updated_at))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id: forwarding_settings_object_id(),
        object_type: "system_settings",
        action: "update",
        before_redacted: before,
        after_redacted: system_settings_audit(transaction).await?,
        created_secret: None,
        reason: None,
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

async fn system_settings_audit(
    transaction: &mut Transaction<'static, Sqlite>,
) -> Result<Value, RepositoryError> {
    let row = sqlx::query_as::<_, SystemSettingsRow>(
        "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=?",
    )
    .bind(FORWARDING_SETTINGS_KEY)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(system_settings_audit_value(&row.into_record()?.value))
}

async fn system_settings_input_for_update(
    transaction: &mut Transaction<'static, Sqlite>,
) -> Result<SystemSettingsInput, RepositoryError> {
    let row = sqlx::query_as::<_, SystemSettingsRow>(
        "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=?",
    )
    .bind(FORWARDING_SETTINGS_KEY)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let settings: SystemSettingsInput = serde_json::from_value(row.into_record()?.value)
        .map_err(|_| RepositoryError::Validation)?;
    validate_system_settings_input(&settings)?;
    Ok(settings)
}

fn forwarding_settings_object_id() -> Uuid {
    Uuid::from_u128(0x6ed3_d02b_bda1_4d85_85b9_3f9d_7362_5001)
}

fn automatic_disable_matches(
    settings: &SystemSettingsInput,
    trigger: &AutomaticDisableTrigger,
) -> bool {
    if !settings.automatic_disable.enabled {
        return false;
    }
    match trigger {
        AutomaticDisableTrigger::HttpStatus(status) => settings
            .automatic_disable
            .error_status_codes
            .contains(status),
        AutomaticDisableTrigger::ErrorMessageKeyword(keyword) => settings
            .automatic_disable
            .error_message_keywords
            .iter()
            .any(|candidate| candidate.trim().to_lowercase() == keyword.to_lowercase()),
    }
}

fn automatic_disable_reason(trigger: &AutomaticDisableTrigger) -> String {
    match trigger {
        AutomaticDisableTrigger::HttpStatus(status) => {
            format!("automatic disable: upstream HTTP status {status}")
        }
        AutomaticDisableTrigger::ErrorMessageKeyword(keyword) => {
            format!("automatic disable: configured error keyword `{keyword}`")
        }
    }
}

async fn automatically_disable_channel(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    trigger: &AutomaticDisableTrigger,
) -> Result<Option<MutationResult>, RepositoryError> {
    let settings = system_settings_input_for_update(transaction).await?;
    if !automatic_disable_matches(&settings, trigger) {
        return Ok(None);
    }
    let before = channel_audit(transaction, id).await?;
    if !before["deleted_at"].is_null()
        || before["enabled"].as_bool() != Some(true)
        || before["auto_disable_allowed"].as_bool() != Some(true)
        || before["auto_disabled"].as_bool() == Some(true)
    {
        return Ok(None);
    }
    let reason = automatic_disable_reason(trigger);
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE channels SET auto_disabled=1,auto_disabled_reason=?,updated_at=ag_now() \
         WHERE id=? AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(&reason)
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(Some(MutationResult {
        id,
        object_type: "channel",
        action: "auto_disable",
        before_redacted: before,
        after_redacted: channel_audit(transaction, id).await?,
        created_secret: None,
        reason: Some(reason),
        updated_at: updated_at.0,
        correlation_id: None,
    }))
}

async fn automatically_recover_channel(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<Option<MutationResult>, RepositoryError> {
    let settings = system_settings_input_for_update(transaction).await?;
    if !settings.scheduled_testing.auto_recover {
        return Ok(None);
    }
    let before = channel_audit(transaction, id).await?;
    if !before["deleted_at"].is_null()
        || before["enabled"].as_bool() != Some(true)
        || before["auto_disabled"].as_bool() != Some(true)
    {
        return Ok(None);
    }
    let reason = "scheduled test succeeded".to_owned();
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE channels SET auto_disabled=0,auto_disabled_reason=NULL,updated_at=ag_now() \
         WHERE id=? AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(Some(MutationResult {
        id,
        object_type: "channel",
        action: "auto_recover",
        before_redacted: before,
        after_redacted: channel_audit(transaction, id).await?,
        created_secret: None,
        reason: Some(reason),
        updated_at: updated_at.0,
        correlation_id: None,
    }))
}

// ---------------------------------------------------------------------------
// Bootstrap initialization
// ---------------------------------------------------------------------------

impl SqliteControlPlaneRepository {
    /// Ensures the hidden, administrator-owned identity used solely by periodic
    /// upstream test request logs. Its API key secret is generated at first
    /// startup and never returned through a management API.
    pub async fn ensure_system_probe_identity(
        &self,
    ) -> Result<SystemProbeIdentity, RepositoryError> {
        let mut transaction = self.write().await?;
        let user_created = sqlx::query_scalar::<_, bool>(
            "INSERT INTO users \
             (id,email,display_name,role,status,balance_amount,user_group_id,is_system) \
             VALUES (?,NULL,?,'admin','active',0,?,1) \
             ON CONFLICT DO NOTHING RETURNING 1",
        )
        .bind(SqliteUuid(SYSTEM_PROBE_USER_ID))
        .bind(SYSTEM_PROBE_DISPLAY_NAME)
        .bind(SqliteUuid(DEFAULT_ADMIN_GROUP_ID))
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
        let key_created = sqlx::query_scalar::<_, i64>(
            "INSERT INTO api_keys \
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions, \
              allowed_group_ids,allowed_channel_ids,is_system) \
             VALUES (?,?,?,?,'active',?,?,'[]','[]',1) \
             ON CONFLICT DO NOTHING RETURNING 1",
        )
        .bind(SqliteUuid(SYSTEM_PROBE_API_KEY_ID))
        .bind(SqliteUuid(SYSTEM_PROBE_USER_ID))
        .bind(SYSTEM_PROBE_API_KEY_NAME)
        .bind(generate_api_key_secret())
        .bind(json_text(&[
            "open_ai_chat_completions",
            "open_ai_responses",
        ])?)
        .bind(json_text(&["proxy", "models.read"])?)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
        let valid = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS( \
                 SELECT 1 FROM api_keys AS key \
                 JOIN users AS user_account ON user_account.id=key.user_id \
                 WHERE key.id=? AND key.user_id=? AND key.is_system=1 \
                   AND key.deleted_at IS NULL AND user_account.is_system=1 \
                   AND user_account.status='active' AND user_account.role='admin' \
                   AND user_account.deleted_at IS NULL)",
        )
        .bind(SqliteUuid(SYSTEM_PROBE_API_KEY_ID))
        .bind(SqliteUuid(SYSTEM_PROBE_USER_ID))
        .fetch_one(&mut *transaction)
        .await?;
        if !valid {
            return Err(RepositoryError::Validation);
        }
        if user_created || key_created {
            sqlx::query(
                "INSERT INTO audit_logs \
                 (id,actor_type,action,object_type,object_id,before_redacted,after_redacted) \
                 VALUES (?,'system','initialize','system_probe_identity',?,'{}',?)",
            )
            .bind(SqliteUuid(Uuid::new_v4()))
            .bind(SqliteUuid(SYSTEM_PROBE_API_KEY_ID))
            .bind(value_to_text(&json!({
                "user_id": SYSTEM_PROBE_USER_ID,
                "api_key_id": SYSTEM_PROBE_API_KEY_ID,
            }))?)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(SystemProbeIdentity {
            user_id: SYSTEM_PROBE_USER_ID,
            api_key_id: SYSTEM_PROBE_API_KEY_ID,
        })
    }

    /// Inserts the first database-backed system policy from bootstrap TOML.
    /// Existing fields are never overwritten; sections introduced after the
    /// original row are filled once from bootstrap values.
    pub async fn ensure_system_settings(
        &self,
        input: SystemSettingsInput,
    ) -> Result<(), RepositoryError> {
        validate_system_settings_input(&input)?;
        let value = serde_json::to_value(&input).expect("system settings serialize");
        let mut transaction = self.write().await?;
        let inserted = sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO system_settings (setting_key,value) VALUES (?,?) \
             ON CONFLICT (setting_key) DO NOTHING RETURNING updated_at",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .bind(value_to_text(&value)?)
        .fetch_optional(&mut *transaction)
        .await?;
        if inserted.is_some() {
            sqlx::query(
                "INSERT INTO audit_logs \
                 (id,actor_type,action,object_type,object_id,before_redacted,after_redacted) \
                 VALUES (?,'system','initialize','system_settings',?,'{}',?)",
            )
            .bind(SqliteUuid(Uuid::new_v4()))
            .bind(SqliteUuid(forwarding_settings_object_id()))
            .bind(value_to_text(&system_settings_audit_value(&value))?)
            .execute(&mut *transaction)
            .await?;
        } else {
            let before = sqlx::query_as::<_, SystemSettingsRow>(
                "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=?",
            )
            .bind(FORWARDING_SETTINGS_KEY)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(RepositoryError::NotFound)?
            .into_record()?
            .value;
            let mut after = before.clone();
            let after_object = after.as_object_mut().ok_or(RepositoryError::Validation)?;
            let mut changed = false;
            for (key, value) in [(
                "codex",
                serde_json::to_value(&input.codex).expect("Codex settings serialize"),
            )] {
                if !after_object.contains_key(key) {
                    after_object.insert(key.into(), value);
                    changed = true;
                }
            }
            if let Some(codex) = after_object
                .get_mut("codex")
                .and_then(serde_json::Value::as_object_mut)
            {
                for (key, value) in [
                    (
                        "originator",
                        serde_json::Value::String(input.codex.originator.clone()),
                    ),
                    (
                        "client_version",
                        serde_json::Value::String(input.codex.client_version.clone()),
                    ),
                    (
                        "user_agent",
                        serde_json::Value::String(input.codex.user_agent.clone()),
                    ),
                ] {
                    if !codex.contains_key(key) {
                        codex.insert(key.into(), value);
                        changed = true;
                    }
                }
            }
            if changed {
                let settings: SystemSettingsInput = serde_json::from_value(after.clone())
                    .map_err(|_| RepositoryError::Validation)?;
                validate_system_settings_input(&settings)?;
                sqlx::query(
                    "UPDATE system_settings SET value=?,updated_at=ag_now() WHERE setting_key=?",
                )
                .bind(value_to_text(&after)?)
                .bind(FORWARDING_SETTINGS_KEY)
                .execute(&mut *transaction)
                .await?;
                sqlx::query(
                    "INSERT INTO audit_logs \
                     (id,actor_type,action,object_type,object_id,before_redacted,after_redacted) \
                     VALUES (?,'system','initialize','system_settings',?,?,?)",
                )
                .bind(SqliteUuid(Uuid::new_v4()))
                .bind(SqliteUuid(forwarding_settings_object_id()))
                .bind(value_to_text(&system_settings_audit_value(&before))?)
                .bind(value_to_text(&system_settings_audit_value(&after))?)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }
}

/// TEXT-backed row for `ControlPlaneModelRuleRow`, whose neutral struct uses
/// plain `Uuid`/`DateTime` fields and cannot be decoded from a SQLite row.
#[derive(FromRow)]
struct ModelRuleListViewRow {
    id: SqliteUuid,
    model_id: SqliteUuid,
    client_model: String,
    model_display_name: String,
    model_provider_name: Option<String>,
    model_enabled: bool,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ModelRuleListViewRow {
    fn into_row(self) -> ControlPlaneModelRuleRow {
        ControlPlaneModelRuleRow {
            id: self.id.0,
            model_id: self.model_id.0,
            client_model: self.client_model,
            model_display_name: self.model_display_name,
            model_provider_name: self.model_provider_name,
            model_enabled: self.model_enabled,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        }
    }
}

/// Builds the serializable model-rule view from a TEXT-backed row. The parent
/// constructor is private to `persistence`, so the projection is repeated here
/// with identical sorting semantics.
fn control_plane_model_rule(
    row: ControlPlaneModelRuleRow,
    mut protocol_rules: Vec<ControlPlaneModelProtocolRule>,
) -> ControlPlaneModelRule {
    protocol_rules.sort_unstable_by(|left, right| left.api_format.cmp(&right.api_format));
    ControlPlaneModelRule {
        id: row.id,
        model_id: row.model_id,
        client_model: row.client_model,
        model_display_name: row.model_display_name,
        model_provider_name: row.model_provider_name,
        model_enabled: row.model_enabled,
        protocol_rules,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Persists one Codex sharing-group policy. The database enforces provider
/// identity uniqueness, seat occupancy, and the monotonic seat-count guard, so
/// the repository validates the API-level shape and binds the credential's
/// provider identity itself.
async fn save_codex_sharing(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
    mut input: crate::domain::codex_sharing::SharingGroupInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if !input.valid() {
        return Err(RepositoryError::Validation);
    }
    input.name = input.name.trim().to_owned();
    let before = sqlx::query_as::<_, SharingGroupAuditRow>(SHARING_GROUP_SELECT)
        .bind(SqliteUuid(id))
        .fetch_optional(&mut **transaction)
        .await?
        .map(SharingGroupAuditRow::into_value)
        .transpose()?;
    match (&before, expected) {
        (Some(value), Some(version)) => {
            let previous: crate::domain::codex_sharing::SharingGroup =
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
    let member_array = uuid_array_text(&members)?;
    let member_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM users WHERE deleted_at IS NULL AND is_system=0 AND status='active' \
           AND EXISTS (SELECT 1 FROM json_each(?) AS member WHERE member.value=users.id)",
    )
    .bind(&member_array)
    .fetch_one(&mut **transaction)
    .await?;
    if member_count != members.len() as i64 {
        return Err(RepositoryError::Validation);
    }
    let identity = sqlx::query_as::<_, (String, String)>(
        "SELECT COALESCE(account_id,''),user_id FROM codex_oauth_credentials \
         WHERE channel_id=? AND deleted_at IS NULL AND user_id IS NOT NULL AND user_id<>''",
    )
    .bind(SqliteUuid(input.credential_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Validation)?;
    let seated_users = input
        .seats
        .iter()
        .flatten()
        .map(Uuid::to_string)
        .collect::<Vec<_>>();
    let seated_array = json_text(&seated_users)?;
    let conflict = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM codex_sharing_groups WHERE id<>? \
         AND (credential_id=? \
              OR (provider_account_id=? AND provider_user_id=?) \
              OR EXISTS (SELECT 1 FROM json_each(codex_sharing_groups.seats) AS member \
                         WHERE EXISTS (SELECT 1 FROM json_each(?) AS target \
                                       WHERE target.value=member.value))))",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(input.credential_id))
    .bind(&identity.0)
    .bind(&identity.1)
    .bind(&seated_array)
    .fetch_one(&mut **transaction)
    .await?;
    if conflict {
        return Err(RepositoryError::Conflict);
    }
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "INSERT INTO codex_sharing_groups \
         (id,credential_id,provider_account_id,provider_user_id,name,enabled,seats, \
          primary_limit_amount,secondary_limit_amount,request_reservation_amount, \
          user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests, \
          group_max_concurrent_requests) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
         ON CONFLICT (id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled, \
          seats=excluded.seats,primary_limit_amount=excluded.primary_limit_amount, \
          secondary_limit_amount=excluded.secondary_limit_amount, \
          request_reservation_amount=excluded.request_reservation_amount, \
          user_requests_per_minute=excluded.user_requests_per_minute, \
          group_requests_per_minute=excluded.group_requests_per_minute, \
          user_max_concurrent_requests=excluded.user_max_concurrent_requests, \
          group_max_concurrent_requests=excluded.group_max_concurrent_requests, \
          updated_at=ag_now() \
         RETURNING updated_at",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteUuid(input.credential_id))
    .bind(&identity.0)
    .bind(&identity.1)
    .bind(&input.name)
    .bind(input.enabled)
    .bind(json_text(&input.seats)?)
    .bind(amount_20_8(input.primary_limit_amount)?)
    .bind(amount_20_8(input.secondary_limit_amount)?)
    .bind(amount_20_8(input.request_reservation_amount)?)
    .bind(i32::try_from(input.user_requests_per_minute).map_err(|_| RepositoryError::Validation)?)
    .bind(i32::try_from(input.group_requests_per_minute).map_err(|_| RepositoryError::Validation)?)
    .bind(
        i32::try_from(input.user_max_concurrent_requests)
            .map_err(|_| RepositoryError::Validation)?,
    )
    .bind(
        i32::try_from(input.group_max_concurrent_requests)
            .map_err(|_| RepositoryError::Validation)?,
    )
    .fetch_one(&mut **transaction)
    .await?;
    let after = serde_json::to_value(crate::domain::codex_sharing::SharingGroup {
        id,
        policy: input,
        updated_at: updated_at.0,
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
        updated_at: updated_at.0,
        correlation_id: None,
    })
}

const SHARING_GROUP_SELECT: &str = "SELECT id,credential_id,provider_account_id,provider_user_id, \
     name,enabled,seats,primary_limit_amount,secondary_limit_amount,request_reservation_amount, \
     user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests, \
     group_max_concurrent_requests,updated_at FROM codex_sharing_groups WHERE id=?";

#[derive(FromRow)]
pub(super) struct SharingGroupAuditRow {
    id: SqliteUuid,
    credential_id: SqliteUuid,
    name: String,
    enabled: bool,
    seats: String,
    primary_limit_amount: SqliteSharingAmount,
    secondary_limit_amount: SqliteSharingAmount,
    request_reservation_amount: SqliteSharingAmount,
    user_requests_per_minute: i32,
    group_requests_per_minute: i32,
    user_max_concurrent_requests: i32,
    group_max_concurrent_requests: i32,
    updated_at: SqliteTimestamp,
}

impl SharingGroupAuditRow {
    pub(super) fn into_value(self) -> Result<Value, RepositoryError> {
        let seats: Vec<Option<Uuid>> =
            serde_json::from_str(&self.seats).map_err(|_| RepositoryError::Validation)?;
        Ok(json!({
            "id": self.id.0,
            "credential_id": self.credential_id.0,
            "name": self.name,
            "enabled": self.enabled,
            "seats": seats,
            "primary_limit_amount": format!("{:.8}",self.primary_limit_amount.0),
            "secondary_limit_amount": format!("{:.8}",self.secondary_limit_amount.0),
            "request_reservation_amount": format!("{:.8}",self.request_reservation_amount.0),
            "user_requests_per_minute": self.user_requests_per_minute,
            "group_requests_per_minute": self.group_requests_per_minute,
            "user_max_concurrent_requests": self.user_max_concurrent_requests,
            "group_max_concurrent_requests": self.group_max_concurrent_requests,
            "updated_at": self.updated_at.0,
        }))
    }
}
