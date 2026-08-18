//! Backend-neutral control-plane persistence contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::records::SystemSettingsInput;

/// Explicit, typed management inputs. HTTP owns request decoding; backend
/// adapters own the fixed statements for these supported resources.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyCreate {
    pub user_id: Uuid,
    pub name: String,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub expires_at: Option<DateTime<Utc>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyUpdate {
    pub name: String,
    pub status: String,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub expires_at: Option<DateTime<Utc>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}

/// Administrator-owned bounds for the routing targets a user may choose.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyPolicyInput {
    pub name: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserGroupInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_api_key_policy_id: Option<Uuid>,
    #[serde(default)]
    pub visible_codex_quota_group_ids: Vec<Uuid>,
    #[serde(default)]
    pub filter_fast_mode: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfApiKeyCreate {
    pub name: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub requests_per_minute: Option<i32>,
    #[serde(default)]
    pub max_concurrent_requests: Option<i32>,
    #[serde(default)]
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfApiKeyUpdate {
    pub name: String,
    pub status: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub requests_per_minute: Option<i32>,
    #[serde(default)]
    pub max_concurrent_requests: Option<i32>,
    #[serde(default)]
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserInput {
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default = "default_user_role")]
    pub role: String,
    pub status: String,
    pub balance_amount: rust_decimal::Decimal,
    #[serde(default)]
    pub user_group_id: Option<Uuid>,
    #[serde(default)]
    pub default_api_key_policy_id: Option<Uuid>,
}

/// Partial administrator update for a Console user.
///
/// Omitted fields preserve their current value. Nullable fields use a nested
/// option so an explicit JSON `null` can clear the value while omission leaves
/// it unchanged.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserUpdateInput {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub email: Option<Option<String>>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub balance_amount: Option<rust_decimal::Decimal>,
    #[serde(default)]
    pub user_group_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_uuid")]
    pub default_api_key_policy_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub websocket_enabled: Option<bool>,
}

impl UserUpdateInput {
    pub(super) fn is_empty(&self) -> bool {
        self.display_name.is_none()
            && self.email.is_none()
            && self.role.is_none()
            && self.status.is_none()
            && self.balance_amount.is_none()
            && self.user_group_id.is_none()
            && self.default_api_key_policy_id.is_none()
            && self.websocket_enabled.is_none()
    }
}

impl From<UserInput> for UserUpdateInput {
    fn from(value: UserInput) -> Self {
        Self {
            display_name: Some(value.display_name),
            email: Some(value.email),
            role: Some(value.role),
            status: Some(value.status),
            balance_amount: Some(value.balance_amount),
            user_group_id: value.user_group_id,
            default_api_key_policy_id: Some(value.default_api_key_policy_id),
            websocket_enabled: None,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBatchUpdateInput {
    pub items: Vec<UserBatchUpdateTarget>,
    pub changes: UserBatchChanges,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBatchUpdateTarget {
    pub id: Uuid,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBalanceBatchChange {
    pub operation: String,
    pub amount: rust_decimal::Decimal,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBatchChanges {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub balance: Option<UserBalanceBatchChange>,
    #[serde(default)]
    pub user_group_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_uuid")]
    pub default_api_key_policy_id: Option<Option<Uuid>>,
}

impl UserBatchChanges {
    pub(super) fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.balance.is_none()
            && self.user_group_id.is_none()
            && self.default_api_key_policy_id.is_none()
    }
}

fn default_user_role() -> String {
    "user".into()
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInput {
    pub source_model_id: String,
    pub display_name: String,
    #[serde(default)]
    pub provider_name: Option<String>,
    pub enabled: bool,
    pub price_unit_tokens: i64,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub price_effective_at: DateTime<Utc>,
    /// Create defaults to no advanced billing; omission during an update
    /// preserves the model's existing policy.
    #[serde(default)]
    pub advanced_billing: Option<crate::domain::AdvancedBilling>,
    /// Create defaults to `{}`; omission during an update preserves the
    /// opaque source document which ordinary reads deliberately do not expose.
    #[serde(default)]
    pub source_payload: Option<Value>,
}
/// A fully validated, price-bearing models.dev catalog entry selected by an
/// administrator. Unlike `ModelInput`, this is not decoded from an HTTP request.
#[derive(Clone)]
pub struct SyncedModelInput {
    pub source_model_id: String,
    pub display_name: String,
    pub provider_name: String,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub advanced_billing: crate::domain::AdvancedBilling,
    pub source_payload: Value,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelGroupInput {
    pub name: String,
    pub api_format: String,
    #[serde(default = "default_connector_kind")]
    pub connector_kind: String,
    /// Create defaults to `default`; omission during an update preserves the
    /// current group-level request compression.
    #[serde(default)]
    pub request_compression: Option<String>,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
    /// Create defaults to disabled; omission during an update preserves the
    /// current group-level status-monitoring setting.
    #[serde(default)]
    pub status_statistics_enabled: Option<bool>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelCreateInput {
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    #[serde(default)]
    pub supports_websocket: bool,
    #[serde(default)]
    pub supports_standalone_web_search: bool,
    #[serde(default)]
    pub auto_disable_allowed: bool,
    pub weight: i32,
    #[serde(default = "default_billing_multiplier")]
    pub billing_multiplier: rust_decimal::Decimal,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    #[serde(default = "empty_object")]
    pub override_document: Value,
    #[serde(default)]
    pub connect_timeout_ms: Option<i32>,
    #[serde(default)]
    pub response_header_timeout_ms: Option<i32>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    #[serde(default)]
    pub upstream_auth_header_name: Option<String>,
    pub upstream_api_key: Option<String>,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default)]
    pub test_model: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelInput {
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    #[serde(default)]
    pub supports_websocket: bool,
    #[serde(default)]
    pub supports_standalone_web_search: bool,
    #[serde(default)]
    pub auto_disable_allowed: bool,
    pub weight: i32,
    /// Omission preserves the current multiplier.
    #[serde(default)]
    pub billing_multiplier: Option<rust_decimal::Decimal>,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    /// Omission preserves the current transform document; a present value
    /// replaces it (including `{}` to clear it).
    #[serde(default, deserialize_with = "deserialize_optional_document")]
    pub override_document: Option<Value>,
    #[serde(default)]
    pub connect_timeout_ms: Option<i32>,
    #[serde(default)]
    pub response_header_timeout_ms: Option<i32>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    #[serde(default)]
    pub upstream_auth_header_name: Option<String>,
    /// Absent keeps the current secret; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub upstream_api_key: Option<Option<String>>,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default)]
    pub test_model: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBatchUpdateInput {
    pub items: Vec<ChannelBatchUpdateTarget>,
    pub changes: ChannelBatchChanges,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBatchUpdateTarget {
    pub id: Uuid,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRecoverInput {
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBatchChanges {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub auto_disable_allowed: Option<bool>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub billing_multiplier: Option<rust_decimal::Decimal>,
}
impl ChannelBatchChanges {
    pub(super) fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.auto_disable_allowed.is_none()
            && self.weight.is_none()
            && self.billing_multiplier.is_none()
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRuleInput {
    pub client_model: String,
    pub api_format: String,
    pub upstream_model_id: Uuid,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub channel_group_ids: Vec<Uuid>,
    #[serde(default)]
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyCreateInput {
    pub name: String,
    pub proxy_url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyInput {
    pub name: String,
    pub proxy_url: String,
    /// Absent keeps the current credential component; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub username: Option<Option<String>>,
    /// Absent keeps the current credential component; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub password: Option<Option<String>>,
    #[serde(default)]
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTemplateCreateInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub document: Value,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTemplateInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Omission preserves the stored document; a present value replaces it.
    #[serde(default, deserialize_with = "deserialize_optional_document")]
    pub document: Option<Value>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerCreateInput {
    pub slug: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub model_rule_id: Uuid,
    #[serde(default = "empty_object")]
    pub settings: Value,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub model_rule_id: Uuid,
    pub settings: Value,
    pub enabled: bool,
}
pub(super) fn empty_object() -> Value {
    json!({})
}
pub(super) fn default_billing_multiplier() -> rust_decimal::Decimal {
    rust_decimal::Decimal::ONE
}
fn default_connector_kind() -> String {
    "openai_compatible".into()
}
fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}
fn deserialize_optional_uuid<'de, D>(deserializer: D) -> Result<Option<Option<Uuid>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Uuid>::deserialize(deserializer).map(Some)
}
fn deserialize_optional_document<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}
pub enum ControlPlaneMutation {
    CreateUser(UserInput),
    UpdateUser {
        id: Uuid,
        input: UserUpdateInput,
        expected_updated_at: DateTime<Utc>,
    },
    DeleteUser {
        id: Uuid,
        deleted_by: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    CreateUserGroup(UserGroupInput),
    UpdateUserGroup {
        id: Uuid,
        input: UserGroupInput,
        expected_updated_at: DateTime<Utc>,
    },
    DeleteUserGroup {
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    CreateModel(ModelInput),
    UpdateModel {
        id: Uuid,
        input: ModelInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateApiKey(ApiKeyCreate),
    CreateApiKeyPolicy(ApiKeyPolicyInput),
    UpdateApiKeyPolicy {
        id: Uuid,
        input: ApiKeyPolicyInput,
        expected_updated_at: DateTime<Utc>,
    },
    UpdateApiKey {
        id: Uuid,
        input: ApiKeyUpdate,
        expected_updated_at: DateTime<Utc>,
    },
    RevokeApiKey {
        id: Uuid,
        reason: String,
    },
    CreateGroup(ChannelGroupInput),
    UpdateGroup {
        id: Uuid,
        input: ChannelGroupInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateChannel(ChannelCreateInput),
    UpdateChannel {
        id: Uuid,
        input: ChannelInput,
        expected_updated_at: DateTime<Utc>,
    },
    RecoverChannel {
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    CreateRule(ModelRuleInput),
    UpdateRule {
        id: Uuid,
        input: ModelRuleInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateProxy(ProxyCreateInput),
    UpdateProxy {
        id: Uuid,
        input: ProxyInput,
        expected_updated_at: DateTime<Utc>,
    },
    DeleteProxy {
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    CreateConfigTemplate(ConfigTemplateCreateInput),
    UpdateConfigTemplate {
        id: Uuid,
        input: ConfigTemplateInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateMcpServer(McpServerCreateInput),
    UpdateMcpServer {
        id: Uuid,
        input: McpServerInput,
        expected_updated_at: DateTime<Utc>,
    },
    DeleteMcpServer {
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    UpdateSystemSettings {
        input: SystemSettingsInput,
        expected_updated_at: DateTime<Utc>,
    },
}

pub struct MutationResult {
    pub id: Uuid,
    pub object_type: &'static str,
    pub action: &'static str,
    pub before_redacted: Value,
    pub after_redacted: Value,
    pub created_secret: Option<String>,
    pub reason: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub correlation_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct ControlPlaneLists {
    pub users: Vec<ControlPlaneUser>,
    pub user_groups: Vec<ControlPlaneUserGroup>,
    pub models: Vec<ControlPlaneModel>,
    pub api_keys: Vec<ControlPlaneApiKey>,
    pub api_key_policies: Vec<ControlPlaneApiKeyPolicy>,
    pub channel_groups: Vec<ControlPlaneChannelGroup>,
    pub channels: Vec<ControlPlaneChannel>,
    pub model_rules: Vec<ControlPlaneModelRule>,
    pub proxies: Vec<ControlPlaneProxy>,
    pub config_templates: Vec<ControlPlaneConfigTemplate>,
    pub mcp_servers: Vec<ControlPlaneMcpServer>,
}
#[derive(Serialize)]
pub struct ControlPlaneUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub can_reissue_invitation: bool,
    pub password_change_required: bool,
    pub temporary_password_expires_at: Option<DateTime<Utc>>,
    pub user_group_id: Uuid,
    pub default_api_key_policy_id: Option<Uuid>,
    pub effective_api_key_policy_id: Option<Uuid>,
    pub websocket_enabled: bool,
    pub balance_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneUserGroup {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub default_api_key_policy_id: Option<Uuid>,
    pub visible_codex_quota_group_ids: Vec<Uuid>,
    pub filter_fast_mode: bool,
    pub system_role: Option<String>,
    pub member_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneModel {
    pub id: Uuid,
    pub source_model_id: String,
    pub display_name: String,
    pub provider_name: Option<String>,
    pub enabled: bool,
    pub price_unit_tokens: i64,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub price_effective_at: DateTime<Utc>,
    pub advanced_billing: Value,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneApiKeyPolicy {
    pub id: Uuid,
    pub name: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct ConsoleApiKey {
    pub id: Uuid,
    pub name: String,
    pub secret: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub quota_used_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct SelfApiKeyOptions {
    pub policy_id: Uuid,
    pub policy_name: String,
    pub groups: Vec<SelfApiKeyGroupOption>,
    pub channels: Vec<SelfApiKeyChannelOption>,
}

#[derive(Clone, Serialize)]
pub struct SelfApiKeyGroupOption {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub priority: i32,
    pub enabled: bool,
}

#[derive(Clone, Serialize)]
pub struct SelfApiKeyChannelOption {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub channel_group_enabled: bool,
    pub api_format: String,
    pub name: String,
    pub enabled: bool,
    pub auto_disabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsoleAuditLog {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_user_id: Option<Uuid>,
    pub actor_type: String,
    pub actor_role: Option<String>,
    pub action: String,
    pub object_type: String,
    pub object_id: Uuid,
    pub before_redacted: Option<Value>,
    pub after_redacted: Option<Value>,
    pub correlation_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct ControlPlaneApiKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_status: String,
    pub name: String,
    pub secret: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub quota_used_amount: rust_decimal::Decimal,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneChannelGroup {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub connector_kind: String,
    pub connector_pool_id: Option<Uuid>,
    pub request_compression: String,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
    pub status_statistics_enabled: bool,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub connector_kind: String,
    pub provider_managed: bool,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub supports_websocket: bool,
    pub supports_standalone_web_search: bool,
    pub auto_disabled: bool,
    pub auto_disabled_reason: Option<String>,
    pub auto_disable_allowed: bool,
    pub weight: i32,
    pub billing_multiplier: rust_decimal::Decimal,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    pub upstream_auth_header_name: Option<String>,
    pub upstream_credential_configured: bool,
    pub available_models: Vec<String>,
    pub test_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneChannelDetail {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub connector_kind: String,
    pub provider_managed: bool,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub supports_websocket: bool,
    pub supports_standalone_web_search: bool,
    pub auto_disabled: bool,
    pub auto_disabled_reason: Option<String>,
    pub auto_disable_allowed: bool,
    pub weight: i32,
    pub billing_multiplier: rust_decimal::Decimal,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub override_document: Value,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    pub upstream_auth_header_name: Option<String>,
    pub upstream_api_key: Option<String>,
    pub upstream_credential_configured: bool,
    pub available_models: Vec<String>,
    pub test_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRuleRoutingStatus {
    Ready,
    TemporarilyUnavailable,
    Disconnected,
    Disabled,
}

#[derive(Serialize)]
pub struct ControlPlaneModelRule {
    pub id: Uuid,
    pub client_model: String,
    pub api_format: String,
    pub upstream_model_id: Uuid,
    pub upstream_model_enabled: bool,
    pub upstream_model: String,
    pub description: Option<String>,
    pub channel_group_ids: Vec<Uuid>,
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
    pub routing_status: ModelRuleRoutingStatus,
    pub target_channel_count: usize,
    pub model_capable_channel_count: usize,
    pub active_channel_count: usize,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ControlPlaneProxy {
    pub id: Uuid,
    pub name: String,
    pub proxy_url: String,
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
    pub credential_configured: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneConfigTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub api_format: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneConfigTemplateDetail {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub api_format: Option<String>,
    pub document: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ControlPlaneMcpServer {
    pub id: Uuid,
    pub slug: String,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub model_rule_id: Uuid,
    pub client_model: String,
    pub api_format: String,
    pub settings_version: i16,
    pub settings: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
