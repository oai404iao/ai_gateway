// Shared Console API fixtures for component and e2e tests.
// Shape comes from the generated types; values are deterministic so tests can
// assert on stable ids/timestamps.

import type {
  ApiKeyPolicyView,
  ApiKeyView,
  ChannelGroupView,
  ChannelView,
  ConfigTemplateView,
  ConsoleProfile,
  ConsoleSession,
  ConsoleUser,
  ControlPlaneModel,
  ControlPlaneUser,
  LoginResponse,
  ModelRuleView,
  ProxyView,
} from "@/api/types";

export const ADMIN_USER: ConsoleUser = {
  id: "00000000-0000-0000-0000-000000000001",
  email: "admin@example.com",
  display_name: "Initial Admin",
  role: "admin",
};

export const ADMIN_PROFILE: ConsoleProfile = {
  id: ADMIN_USER.id,
  email: ADMIN_USER.email,
  display_name: ADMIN_USER.display_name,
  role: "admin",
  status: "active",
  balance_amount: "12.50",
  currency: "USD",
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const ADMIN_ACCESS_TOKEN = "test-access-token-admin";

export const ADMIN_LOGIN_RESPONSE: LoginResponse = {
  access_token: ADMIN_ACCESS_TOKEN,
  token_type: "Bearer",
  expires_in: 900,
  user: ADMIN_USER,
};

export const ACTIVE_SESSION: ConsoleSession = {
  id: "00000000-0000-0000-0000-0000000000a1",
  created_at: "2026-01-10T10:00:00.000Z",
  last_seen_at: "2026-01-10T11:00:00.000Z",
  expires_at: "2026-02-09T10:00:00.000Z",
  revoked_at: null,
};

export const REVOKED_SESSION: ConsoleSession = {
  id: "00000000-0000-0000-0000-0000000000a2",
  created_at: "2026-01-05T10:00:00.000Z",
  last_seen_at: null,
  expires_at: "2026-01-12T10:00:00.000Z",
  revoked_at: "2026-01-06T10:00:00.000Z",
};

export const API_KEY_POLICY: ApiKeyPolicyView = {
  id: "00000000-0000-0000-0000-000000000031",
  name: "default",
  allowed_api_formats: ["open_ai_chat_completions", "open_ai_responses"],
  permissions: ["proxy", "models.read"],
  allowed_group_ids: null,
  requests_per_minute: 60,
  max_concurrent_requests: 4,
  quota_limit_amount: "10.00",
  max_active_keys: 2,
  enabled: true,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const OWN_API_KEY: ApiKeyView = {
  id: "00000000-0000-0000-0000-000000000011",
  name: "dev key",
  status: "active",
  expires_at: "2027-01-01T00:00:00.000Z",
  allowed_api_formats: ["open_ai_chat_completions"],
  permissions: ["proxy"],
  allowed_group_ids: null,
  requests_per_minute: 60,
  max_concurrent_requests: 4,
  quota_limit_amount: "10.00",
  quota_used_amount: "1.25",
  created_at: "2026-01-03T00:00:00.000Z",
  updated_at: "2026-01-03T00:00:00.000Z",
};

export const NEW_API_KEY_SECRET = "sk-ag-test-secret-only-once";

export const CHANNEL_GROUP: ChannelGroupView = {
  id: "00000000-0000-0000-0000-000000000021",
  name: "chat-primary",
  api_format: "open_ai_chat_completions",
  priority: 1,
  selection_strategy: "weighted_random",
  enabled: true,
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const CHANNEL: ChannelView = {
  id: "00000000-0000-0000-0000-000000000022",
  channel_group_id: CHANNEL_GROUP.id,
  api_format: "open_ai_chat_completions",
  name: "upstream-a",
  base_url: "https://api.upstream.example",
  enabled: true,
  auto_disabled: false,
  auto_disabled_reason: null,
  weight: 100,
  proxy_id: null,
  config_template_id: null,
  connect_timeout_ms: null,
  response_header_timeout_ms: null,
  stream_idle_timeout_ms: null,
  upstream_auth_kind: "bearer",
  upstream_auth_header_name: null,
  upstream_credential_configured: true,
  available_models: ["gpt-4o-mini"],
  created_at: "2026-01-02T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const MODEL: ControlPlaneModel = {
  id: "00000000-0000-0000-0000-000000000030",
  source_model_id: "openai/gpt-4o-mini",
  display_name: "GPT-4o mini",
  provider_name: "OpenAI",
  enabled: true,
  currency: "USD",
  price_unit_tokens: 1_000_000,
  input_unit_price: "0.15",
  cached_input_unit_price: "0.075",
  cache_write_unit_price: "0.3",
  output_unit_price: "0.6",
  price_effective_at: "2026-01-01T00:00:00.000Z",
  last_synced_at: null,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-01T00:00:00.000Z",
};

export const MODEL_RULE: ModelRuleView = {
  id: "00000000-0000-0000-0000-000000000025",
  client_model: "gateway-chat-model",
  api_format: "open_ai_chat_completions",
  model_id: MODEL.id,
  model_enabled: true,
  upstream_model: "gpt-4o-mini",
  description: null,
  channel_group_ids: [CHANNEL_GROUP.id],
  channel_ids: [CHANNEL.id],
  enabled: true,
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const PROXY: ProxyView = {
  id: "00000000-0000-0000-0000-000000000026",
  name: "egress-1",
  proxy_url: "http://proxy.example:1080",
  no_proxy_hosts: ["internal.example"],
  enabled: true,
  credential_configured: false,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-01T00:00:00.000Z",
};

export const CONFIG_TEMPLATE: ConfigTemplateView = {
  id: "00000000-0000-0000-0000-000000000027",
  name: "default-transform",
  description: "Default constrained transform document",
  enabled: true,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const CONTROL_PLANE_USER: ControlPlaneUser = {
  id: ADMIN_USER.id,
  email: ADMIN_USER.email,
  display_name: ADMIN_USER.display_name,
  role: "admin",
  status: "active",
  default_api_key_policy_id: API_KEY_POLICY.id,
  balance_amount: "12.50",
  currency: "USD",
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};
