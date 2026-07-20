// Console API TypeScript contracts.
//
// These types mirror the Rust DTOs in `src/persistence/mod.rs`,
// `src/persistence/auth.rs`, and `src/http/console.rs`. rust_decimal::Decimal
// serializes as a string, and chrono::DateTime<Utc> serializes as an RFC 3339
// string, so every decimal and timestamp field is typed as `string` here.
//
// Until an OpenAPI generator is wired into `pnpm generate:api` (see
// docs/console-ui-design.md §7.1), this module is the single source of truth
// for request/response shapes. Do not duplicate these shapes inside features.

export type UserRole = "user" | "admin";

export type ApiFormat = "open_ai_chat_completions" | "open_ai_responses";

export type SelectionStrategy = "priority_weighted" | "weighted";

export type UpstreamAuthKind = "bearer" | "header";

/** `{ error: "..." }` body returned by every Console error response. */
export interface ErrorBody {
  error: string;
}

export interface ConsoleUser {
  id: string;
  email: string;
  display_name: string;
  role: UserRole;
}

export interface LoginResponse {
  access_token: string;
  token_type: "Bearer";
  expires_in: number;
  user: ConsoleUser;
}

export interface ConsoleProfile {
  id: string;
  email: string | null;
  display_name: string;
  role: UserRole;
  status: string;
  balance_amount: string;
  currency: string;
  created_at: string;
  updated_at: string;
}

export interface ConsoleSession {
  id: string;
  created_at: string;
  last_seen_at: string | null;
  expires_at: string;
  revoked_at: string | null;
}

export interface InvitationResponse {
  id: string;
  user_id: string;
  invitation_token: string;
  expires_at: string;
  correlation_id: string;
}

export interface MutationResponse {
  id: string;
  secret?: string;
  correlation_id: string;
}

export interface ReloadResponse {
  correlation_id: string;
}

/** Shared by both own and admin API keys. The admin flavor adds user_id. */
export interface ApiKeyView {
  id: string;
  name: string;
  status: string;
  expires_at: string | null;
  allowed_api_formats: ApiFormat[];
  permissions: string[];
  allowed_group_ids: string[] | null;
  requests_per_minute: number | null;
  max_concurrent_requests: number | null;
  quota_limit_amount: string | null;
  quota_used_amount: string;
  created_at: string;
  updated_at: string;
}

export interface AdminApiKeyView extends ApiKeyView {
  user_id: string;
  user_status: string;
  tokens_per_minute: number | null;
}

export interface ApiKeyPolicyView {
  id: string;
  name: string;
  allowed_api_formats: ApiFormat[];
  permissions: string[];
  allowed_group_ids: string[] | null;
  requests_per_minute: number | null;
  max_concurrent_requests: number | null;
  quota_limit_amount: string | null;
  max_active_keys: number;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface ControlPlaneUser {
  id: string;
  email: string | null;
  display_name: string;
  role: UserRole;
  status: string;
  default_api_key_policy_id: string | null;
  balance_amount: string;
  currency: string;
  created_at: string;
  updated_at: string;
}

export interface ControlPlaneModel {
  id: string;
  source_model_id: string;
  display_name: string;
  provider_name: string | null;
  enabled: boolean;
  currency: string;
  price_unit_tokens: number;
  input_unit_price: string;
  cached_input_unit_price: string;
  cache_write_unit_price: string;
  output_unit_price: string;
  price_effective_at: string;
  last_synced_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface ChannelGroupView {
  id: string;
  name: string;
  api_format: ApiFormat;
  priority: number;
  selection_strategy: SelectionStrategy;
  enabled: boolean;
  updated_at: string;
}

export interface ChannelView {
  id: string;
  channel_group_id: string;
  api_format: ApiFormat;
  name: string;
  base_url: string;
  enabled: boolean;
  auto_disabled: boolean;
  auto_disabled_reason: string | null;
  weight: number;
  proxy_id: string | null;
  config_template_id: string | null;
  connect_timeout_ms: number | null;
  response_header_timeout_ms: number | null;
  stream_idle_timeout_ms: number | null;
  upstream_auth_kind: UpstreamAuthKind;
  upstream_auth_header_name: string | null;
  upstream_credential_configured: boolean;
  available_models: string[];
  created_at: string;
  updated_at: string;
}

export interface ModelRuleView {
  id: string;
  client_model: string;
  api_format: ApiFormat;
  model_id: string;
  model_enabled: boolean;
  upstream_model: string;
  description: string | null;
  channel_group_ids: string[];
  channel_ids: string[];
  enabled: boolean;
  updated_at: string;
}

export interface ProxyView {
  id: string;
  name: string;
  proxy_url: string;
  no_proxy_hosts: string[];
  enabled: boolean;
  credential_configured: boolean;
  created_at: string;
  updated_at: string;
}

export interface ConfigTemplateView {
  id: string;
  name: string;
  description: string | null;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface ControlPlaneLists {
  users: ControlPlaneUser[];
  models: ControlPlaneModel[];
  api_keys: AdminApiKeyView[];
  api_key_policies: ApiKeyPolicyView[];
  channel_groups: ChannelGroupView[];
  channels: ChannelView[];
  model_rules: ModelRuleView[];
  proxies: ProxyView[];
  config_templates: ConfigTemplateView[];
}

export interface RequestLogView {
  id: string;
  started_at: string;
  completed_at: string;
  user_id: string;
  api_key_id: string;
  api_format: ApiFormat;
  client_model: string;
  upstream_model: string | null;
  model_rule_id: string | null;
  channel_group_id: string | null;
  channel_id: string | null;
  outcome: string;
  response_status_code: number | null;
  streamed: boolean;
  ttft_ms: number | null;
  total_duration_ms: number | null;
  input_tokens: number | null;
  cached_input_tokens: number | null;
  cache_write_tokens: number | null;
  output_tokens: number | null;
  currency: string | null;
  cost_amount: string | null;
  error_code: string | null;
  billed_at: string | null;
}

export interface AuditLogView {
  id: string;
  occurred_at: string;
  actor_user_id: string | null;
  actor_type: string;
  actor_role: string | null;
  action: string;
  object_type: string;
  object_id: string;
  before_redacted: unknown;
  after_redacted: unknown;
  correlation_id: string | null;
  reason: string | null;
}

export type ModelSyncAction = "price_update" | "import" | "already_exists";

export interface ModelSyncPreviewModel {
  provider_id: string;
  provider_name: string;
  model_id: string;
  display_name: string;
  input_unit_price: string;
  cached_input_unit_price: string;
  cache_write_unit_price: string;
  output_unit_price: string;
  action: ModelSyncAction;
}

export interface ModelSyncPreview {
  fetched_at: string;
  models: ModelSyncPreviewModel[];
  excluded_missing_prices: number;
  excluded_invalid_models: number;
  excluded_oversized_metadata: number;
  unavailable_existing_count: number;
}

export interface ModelPriceSyncResponse {
  updated_count: number;
  unavailable_count: number;
  correlation_id: string | null;
}

export interface ModelImportResponse {
  model_count: number;
  correlation_id: string;
}

// ---- Request bodies (subset used by the UI) ----

export interface LoginInput {
  email: string;
  password: string;
}

export interface ActivateInvitationInput {
  invitation_token: string;
  password: string;
}

export interface ProfileUpdateInput {
  display_name: string;
}

export interface PasswordChangeInput {
  current_password: string;
  new_password: string;
}

export interface RevokeInput {
  reason: string;
}

export interface SelfApiKeyCreateInput {
  name: string;
  expires_at?: string | null;
}

export interface SelfApiKeyUpdateInput {
  name: string;
  status: string;
  expires_at?: string | null;
}

export interface InviteUserInput {
  email: string;
  display_name: string;
  role: UserRole;
  currency: string;
  default_api_key_policy_id?: string | null;
}

export interface UserInput {
  display_name: string;
  email?: string | null;
  role: UserRole;
  status: string;
  currency: string;
  default_api_key_policy_id?: string | null;
}

export interface ApiKeyPolicyInput {
  name: string;
  allowed_api_formats: ApiFormat[];
  permissions: string[];
  allowed_group_ids?: string[] | null;
  requests_per_minute?: number | null;
  max_concurrent_requests?: number | null;
  quota_limit_amount?: string | null;
  max_active_keys: number;
  enabled: boolean;
}

export interface ApiKeyCreateInput {
  user_id: string;
  name: string;
  allowed_api_formats: ApiFormat[];
  permissions: string[];
  allowed_group_ids?: string[] | null;
  expires_at?: string | null;
  requests_per_minute?: number | null;
  max_concurrent_requests?: number | null;
  quota_limit_amount?: string | null;
}

export interface ApiKeyUpdateInput {
  name: string;
  status: string;
  allowed_api_formats: ApiFormat[];
  permissions: string[];
  allowed_group_ids?: string[] | null;
  expires_at?: string | null;
  requests_per_minute?: number | null;
  max_concurrent_requests?: number | null;
  quota_limit_amount?: string | null;
}

export interface ModelInput {
  source_model_id: string;
  display_name: string;
  provider_name?: string | null;
  enabled: boolean;
  currency: string;
  price_unit_tokens: number;
  input_unit_price: string;
  cached_input_unit_price: string;
  cache_write_unit_price: string;
  output_unit_price: string;
  price_effective_at: string;
  source_payload?: unknown;
}

export interface ChannelGroupInput {
  name: string;
  api_format: ApiFormat;
  priority: number;
  selection_strategy: SelectionStrategy;
  enabled: boolean;
}

export interface ChannelCreateInput {
  channel_group_id: string;
  api_format: ApiFormat;
  name: string;
  base_url: string;
  enabled: boolean;
  weight: number;
  proxy_id?: string | null;
  config_template_id?: string | null;
  override_document?: unknown;
  connect_timeout_ms?: number | null;
  response_header_timeout_ms?: number | null;
  stream_idle_timeout_ms?: number | null;
  upstream_auth_kind: UpstreamAuthKind;
  upstream_auth_header_name?: string | null;
  upstream_api_key?: string | null;
  available_models?: string[];
  health_check?: unknown;
}

export interface ChannelInput {
  channel_group_id: string;
  api_format: ApiFormat;
  name: string;
  base_url: string;
  enabled: boolean;
  weight: number;
  proxy_id?: string | null;
  config_template_id?: string | null;
  override_document?: unknown;
  connect_timeout_ms?: number | null;
  response_header_timeout_ms?: number | null;
  stream_idle_timeout_ms?: number | null;
  upstream_auth_kind: UpstreamAuthKind;
  upstream_auth_header_name?: string | null;
  upstream_api_key?: string | null;
  available_models?: string[];
  health_check?: unknown;
}

export interface ModelRuleInput {
  client_model: string;
  api_format: ApiFormat;
  model_id: string;
  upstream_model: string;
  description?: string | null;
  channel_group_ids?: string[];
  channel_ids?: string[];
  enabled: boolean;
}

export interface ProxyCreateInput {
  name: string;
  proxy_url: string;
  username?: string | null;
  password?: string | null;
  no_proxy_hosts?: string[];
  enabled: boolean;
}

export interface ProxyInput {
  name: string;
  proxy_url: string;
  username?: string | null;
  password?: string | null;
  no_proxy_hosts?: string[];
  enabled: boolean;
}

export interface ConfigTemplateInput {
  name: string;
  description?: string | null;
  document: unknown;
  enabled: boolean;
}

export interface ModelSyncSelection {
  provider_id: string;
  model_id: string;
}

export interface ModelImportRequest {
  selections: ModelSyncSelection[];
}

export interface ModelSyncPreviewRequest {
  provider_ids?: string[];
}

export interface ModelPriceSyncRequest {
  provider_ids?: string[];
}

export interface ListQuery {
  limit?: number;
}
