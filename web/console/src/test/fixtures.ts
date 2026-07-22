// Shared Console API fixtures for component and e2e tests.
// Shape comes from the generated types; values are deterministic so tests can
// assert on stable ids/timestamps.

import type {
  AdminApiKeyView,
  ApiKeyPolicyView,
  ApiKeyView,
  ChannelStatusReport,
  ChannelGroupView,
  ChannelView,
  ConfigTemplateView,
  ConsoleProfile,
  ConsoleSession,
  ConsoleUser,
  CostStatisticsReport,
  ControlPlaneModel,
  ControlPlaneUser,
  LoginResponse,
  ModelRuleView,
  ProxyView,
  RequestLogView,
  SelfApiKeyOptions,
  SystemSettings,
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
  allowed_group_ids: ["00000000-0000-0000-0000-000000000021"],
  allowed_channel_ids: [],
  enabled: true,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const NEW_API_KEY_SECRET = "sk-ag-test-secret-retrievable";

export const OWN_API_KEY: ApiKeyView = {
  id: "00000000-0000-0000-0000-000000000011",
  name: "dev key",
  secret: NEW_API_KEY_SECRET,
  status: "active",
  expires_at: "2027-01-01T00:00:00.000Z",
  allowed_api_formats: ["open_ai_chat_completions"],
  permissions: ["proxy", "models.read"],
  allowed_group_ids: ["00000000-0000-0000-0000-000000000021"],
  allowed_channel_ids: [],
  requests_per_minute: 60,
  max_concurrent_requests: 4,
  quota_limit_amount: "10.00",
  quota_used_amount: "1.25",
  created_at: "2026-01-03T00:00:00.000Z",
  updated_at: "2026-01-03T00:00:00.000Z",
};

export const ADMIN_API_KEY: AdminApiKeyView = {
  ...OWN_API_KEY,
  user_id: ADMIN_USER.id,
  user_status: "active",
  tokens_per_minute: 10_000,
};

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
  status_statistics_enabled: true,
  auto_disabled: false,
  auto_disabled_reason: null,
  auto_disable_allowed: true,
  weight: 100,
  proxy_id: null,
  config_template_id: null,
  connect_timeout_ms: null,
  response_header_timeout_ms: null,
  stream_idle_timeout_ms: null,
  upstream_auth_kind: "bearer",
  upstream_auth_header_name: null,
  upstream_credential_configured: true,
  available_models: ["openai/gpt-4o-mini"],
  test_model: "openai/gpt-4o-mini",
  created_at: "2026-01-02T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const API_KEY_OPTIONS: SelfApiKeyOptions = {
  policy_id: API_KEY_POLICY.id,
  policy_name: API_KEY_POLICY.name,
  groups: [
    {
      id: CHANNEL_GROUP.id,
      name: CHANNEL_GROUP.name,
      api_format: CHANNEL_GROUP.api_format,
      enabled: CHANNEL_GROUP.enabled,
    },
  ],
  channels: [
    {
      id: CHANNEL.id,
      channel_group_id: CHANNEL.channel_group_id,
      channel_group_name: CHANNEL_GROUP.name,
      api_format: CHANNEL.api_format,
      name: CHANNEL.name,
      enabled: CHANNEL.enabled,
      auto_disabled: CHANNEL.auto_disabled,
    },
  ],
};

export const MODEL: ControlPlaneModel = {
  id: "00000000-0000-0000-0000-000000000030",
  source_model_id: "openai/gpt-4o-mini",
  display_name: "GPT-4o mini",
  provider_name: "OpenAI",
  enabled: true,
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
  upstream_model_id: MODEL.id,
  upstream_model_enabled: true,
  upstream_model: MODEL.source_model_id,
  description: null,
  channel_group_ids: [CHANNEL_GROUP.id],
  channel_ids: [],
  enabled: true,
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const REQUEST_LOG: RequestLogView = {
  id: "11111111-2222-4333-8444-555555555555",
  started_at: "2026-07-21T06:00:00Z",
  completed_at: "2026-07-21T06:00:01Z",
  user_id: ADMIN_USER.id,
  api_key_id: OWN_API_KEY.id,
  request_source: "client",
  api_format: "open_ai_chat_completions",
  client_model: MODEL_RULE.client_model,
  upstream_model: MODEL.source_model_id,
  model_rule_id: MODEL_RULE.id,
  channel_group_id: CHANNEL_GROUP.id,
  channel_id: CHANNEL.id,
  outcome: "succeeded",
  response_status_code: 200,
  streamed: true,
  ttft_ms: 100,
  total_duration_ms: 1_000,
  input_tokens: 12,
  cached_input_tokens: 2,
  cache_write_tokens: 0,
  output_tokens: 4,
  cost_amount: "0.0001",
  error_code: null,
  error_summary: null,
  billed_at: "2026-07-21T06:00:02Z",
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
  api_format: "open_ai_chat_completions",
  enabled: true,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const SYSTEM_SETTINGS: SystemSettings = {
  upstream: {
    connect_timeout_seconds: 10,
    response_header_timeout_seconds: 30,
    stream_idle_timeout_seconds: 90,
  },
  request_retry: {
    enabled: true,
    max_attempts: 2,
  },
  passive_health: {
    connection_failure_threshold: 3,
    cooldown_seconds: 30,
  },
  automatic_disable: {
    enabled: true,
    error_status_codes: [429, 500],
    error_message_keywords: ["quota exceeded"],
  },
  scheduled_testing: {
    mode: "global",
    auto_recover: true,
    interval_minutes: 5,
    prompt: "reply '1'",
  },
  session_affinity: {
    enabled: false,
    max_entries: 100_000,
    default_ttl_seconds: 3_600,
    rules: [],
  },
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
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const CHANNEL_STATUS_REPORT: ChannelStatusReport = {
  window: "24h",
  started_at: "2026-07-20T14:00:00.000Z",
  ended_at: "2026-07-21T13:46:00.000Z",
  bucket_seconds: 1_800,
  models: [
    {
      api_format: "open_ai_chat_completions",
      model: MODEL.source_model_id,
      request_count: 120,
      success_rate: 0.975,
      p90_ttft_ms: 540,
      p50_tps: 31.2,
    },
  ],
  channels: [
    {
      id: CHANNEL.id,
      channel_group_id: CHANNEL.channel_group_id,
      channel_group_name: CHANNEL_GROUP.name,
      api_format: CHANNEL.api_format,
      name: CHANNEL.name,
      enabled: CHANNEL.enabled,
      auto_disabled: CHANNEL.auto_disabled,
      models: [
        {
          api_format: CHANNEL.api_format,
          model: MODEL.source_model_id,
          request_count: 120,
          success_rate: 0.975,
          p90_ttft_ms: 540,
          p50_tps: 31.2,
          history: [
            {
              started_at: "2026-07-21T13:30:00.000Z",
              request_count: 4,
              success_rate: 1,
              p90_ttft_ms: 510,
              p50_tps: 32,
            },
          ],
        },
      ],
    },
  ],
};

export const COST_STATISTICS_REPORT: CostStatisticsReport = {
  started_at: "2026-07-14T14:00:00.000Z",
  ended_at: "2026-07-21T14:00:00.000Z",
  granularity: "day",
  summary: {
    request_count: 18_878,
    priced_request_count: 18_800,
    total_tokens: 263_000_000,
    average_rpm: 1.87,
    average_tpm: 26_100,
    cost_amount: "1912.06",
  },
  buckets: [
    {
      started_at: "2026-07-20T00:00:00.000Z",
      request_count: 1_200,
      total_tokens: 16_000_000,
      cost_amount: "120.5",
      models: [
        {
          api_format: "open_ai_chat_completions",
          model: MODEL.source_model_id,
          request_count: 1_200,
          total_tokens: 16_000_000,
          cost_amount: "120.5",
        },
      ],
    },
    {
      started_at: "2026-07-21T00:00:00.000Z",
      request_count: 1_500,
      total_tokens: 18_000_000,
      cost_amount: "146.75",
      models: [
        {
          api_format: "open_ai_chat_completions",
          model: MODEL.source_model_id,
          request_count: 1_500,
          total_tokens: 18_000_000,
          cost_amount: "146.75",
        },
      ],
    },
  ],
  models: [
    {
      api_format: "open_ai_chat_completions",
      model: MODEL.source_model_id,
      request_count: 18_878,
      total_tokens: 263_000_000,
      success_rate: 0.975,
      cost_amount: "1912.06",
    },
  ],
};
