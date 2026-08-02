// Shared Console API fixtures for component and e2e tests.
// Shape comes from the generated types; values are deterministic so tests can
// assert on stable ids/timestamps.

import type {
  AdminApiKeyView,
  ApiKeyPolicyView,
  ApiKeyView,
  ChannelDetailView,
  ChannelStatusReport,
  ChannelGroupView,
  ChannelView,
  ConfigTemplateDetailView,
  ConfigTemplateView,
  ConsoleProfile,
  ConsoleSession,
  ConsoleUser,
  CostStatisticsReport,
  ControlPlaneModel,
  ControlPlaneUser,
  LoginResponse,
  ModelRuleView,
  PersonalUsageReport,
  ProxyTestResponse,
  ProxyView,
  RegistrationInvitationCodeView,
  RequestLogView,
  SelfApiKeyOptions,
  SessionAffinityCacheReport,
  SpendLeaderboardReport,
  SystemLoadReport,
  SystemSettings,
  UserGroupView,
  UserSettings,
} from "@/api/types";

export const ADMIN_USER: ConsoleUser = {
  id: "00000000-0000-0000-0000-000000000001",
  email: "admin@example.com",
  display_name: "Initial Admin",
  role: "admin",
  password_change_required: false,
  temporary_password_expires_at: null,
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

export const USER_SETTINGS: UserSettings = {
  websocket_enabled: false,
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const ADMIN_ACCESS_TOKEN = "test-access-token-admin";

export const USER_USER: ConsoleUser = {
  id: "00000000-0000-0000-0000-000000000002",
  email: "user@example.com",
  display_name: "Console User",
  role: "user",
  password_change_required: false,
  temporary_password_expires_at: null,
};

export const USER_ACCESS_TOKEN = "test-access-token-user";

export const TEMPORARY_PASSWORD_USER: ConsoleUser = {
  ...USER_USER,
  id: "00000000-0000-0000-0000-000000000003",
  email: "reset@example.com",
  display_name: "Password Reset User",
  password_change_required: true,
  temporary_password_expires_at: "2099-08-02T00:00:00.000Z",
};

export const TEMPORARY_PASSWORD_ACCESS_TOKEN = "test-password-change-token";

export const TEMPORARY_PASSWORD_LOGIN_RESPONSE: LoginResponse = {
  access_token: TEMPORARY_PASSWORD_ACCESS_TOKEN,
  token_type: "Bearer",
  expires_in: 900,
  user: TEMPORARY_PASSWORD_USER,
};

export const ADMIN_LOGIN_RESPONSE: LoginResponse = {
  access_token: ADMIN_ACCESS_TOKEN,
  token_type: "Bearer",
  expires_in: 900,
  user: ADMIN_USER,
};

export const ACTIVE_SESSION: ConsoleSession = {
  id: "00000000-0000-0000-0000-0000000000a1",
  user_agent:
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15",
  created_at: "2026-07-27T08:00:00.000Z",
  last_seen_at: "2026-07-27T10:00:00.000Z",
  expires_at: "2099-08-26T08:00:00.000Z",
  revoked_at: null,
  state: "active",
  is_current: true,
};

export const OTHER_ACTIVE_SESSION: ConsoleSession = {
  id: "00000000-0000-0000-0000-0000000000a3",
  user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Firefox/128.0",
  created_at: "2026-07-26T08:00:00.000Z",
  last_seen_at: "2026-07-27T09:00:00.000Z",
  expires_at: "2099-08-25T08:00:00.000Z",
  revoked_at: null,
  state: "active",
  is_current: false,
};

export const REVOKED_SESSION: ConsoleSession = {
  id: "00000000-0000-0000-0000-0000000000a2",
  user_agent: "curl/8.7.1 (Linux)",
  created_at: "2026-01-05T10:00:00.000Z",
  last_seen_at: "2026-01-05T10:00:00.000Z",
  expires_at: "2026-01-12T10:00:00.000Z",
  revoked_at: "2026-01-06T10:00:00.000Z",
  state: "revoked",
  is_current: false,
};

export const EXPIRED_SESSION: ConsoleSession = {
  id: "00000000-0000-0000-0000-0000000000a4",
  user_agent: null,
  created_at: "2026-01-01T10:00:00.000Z",
  last_seen_at: "2026-01-01T10:00:00.000Z",
  expires_at: "2026-01-02T10:00:00.000Z",
  revoked_at: null,
  state: "expired",
  is_current: false,
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

export const DEFAULT_USER_GROUP: UserGroupView = {
  id: "00000000-0000-0000-0000-000000000101",
  name: "Default Users",
  description: "Default group for newly invited users.",
  default_api_key_policy_id: API_KEY_POLICY.id,
  system_role: "user",
  member_count: 1,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const USER_GROUP: UserGroupView = {
  id: "00000000-0000-0000-0000-000000000102",
  name: "Default Administrators",
  description: "Default group for newly invited administrators.",
  default_api_key_policy_id: API_KEY_POLICY.id,
  system_role: "admin",
  member_count: 1,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const REGISTRATION_INVITATION_CODE: RegistrationInvitationCodeView = {
  id: "00000000-0000-0000-0000-0000000000c1",
  name: "Community launch",
  max_uses: 100,
  used_count: 12,
  expires_at: "2030-01-01T00:00:00.000Z",
  enabled: true,
  user_group_id: DEFAULT_USER_GROUP.id,
  initial_balance_amount: "20.00",
  created_by: ADMIN_USER.id,
  last_used_at: "2026-07-26T12:00:00.000Z",
  created_at: "2026-07-01T00:00:00.000Z",
  updated_at: "2026-07-26T12:00:00.000Z",
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
};

export const CHANNEL_GROUP: ChannelGroupView = {
  id: "00000000-0000-0000-0000-000000000021",
  name: "chat-primary",
  api_format: "open_ai_chat_completions",
  connector_kind: "openai_compatible",
  priority: 1,
  selection_strategy: "weighted_random",
  enabled: true,
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const CHANNEL: ChannelView = {
  id: "00000000-0000-0000-0000-000000000022",
  channel_group_id: CHANNEL_GROUP.id,
  api_format: "open_ai_chat_completions",
  connector_kind: "openai_compatible",
  provider_managed: false,
  name: "upstream-a",
  base_url: "https://api.upstream.example",
  enabled: true,
  supports_websocket: false,
  status_statistics_enabled: true,
  auto_disabled: false,
  auto_disabled_reason: null,
  auto_disable_allowed: true,
  weight: 100,
  billing_multiplier: "1.25",
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

export const CHANNEL_DETAIL: ChannelDetailView = {
  ...CHANNEL,
  override_document: {
    version: 1,
    api_format: "open_ai_chat_completions",
    request_headers: {
      set: {
        "x-channel-source": "console-test",
      },
    },
  },
  upstream_api_key: "sk-upstream-test-secret",
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
  advanced_billing: {
    long_context_tiers: [],
    request_multipliers: [],
  },
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
  user_name: ADMIN_USER.display_name,
  api_key_id: OWN_API_KEY.id,
  request_source: "client",
  api_format: "open_ai_chat_completions",
  api_operation: "chat_completions",
  request_protocol: "sse",
  client_model: MODEL_RULE.client_model,
  reasoning_effort: "high",
  fast_mode: true,
  upstream_model: MODEL.source_model_id,
  model_rule_id: MODEL_RULE.id,
  channel_group_id: CHANNEL_GROUP.id,
  channel_group_name: CHANNEL_GROUP.name,
  channel_id: CHANNEL.id,
  channel_name: CHANNEL.name,
  outcome: "succeeded",
  response_status_code: 200,
  streamed: true,
  ttft_ms: 100,
  total_duration_ms: 1_000,
  output_tokens_per_second: "4.4444",
  input_tokens: 12,
  cached_input_tokens: 2,
  cache_write_tokens: 0,
  output_tokens: 4,
  reasoning_tokens: 1,
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

export const PROXY_TEST_RESULT: ProxyTestResponse = {
  ip: "203.0.113.10",
  continent: "North America",
  continent_code: "NA",
  country: "United States",
  country_code: "US",
  region_code: "CA",
  region_name: "California",
  city: "Los Angeles",
  district: null,
  postal_code: "90001",
  latitude: 34.0522,
  longitude: -118.2437,
  timezone: "America/Los_Angeles",
  utc_offset_seconds: -25_200,
  currency: "USD",
  isp: "Example ISP",
  organization: "Example Organization",
  autonomous_system: "AS64500 Example",
  autonomous_system_name: "EXAMPLE",
  mobile: false,
  proxy: true,
  hosting: false,
  latency_ms: 42,
  rate_limit_remaining: 44,
  rate_limit_reset_seconds: 60,
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

export const CONFIG_TEMPLATE_DETAIL: ConfigTemplateDetailView = {
  ...CONFIG_TEMPLATE,
  document: {
    version: 1,
    api_format: "open_ai_chat_completions",
    request_headers: {
      set: {
        "x-template-source": "console-test",
      },
    },
  },
};

export const SYSTEM_SETTINGS: SystemSettings = {
  api_hosts: ["https://api.example.test/v1"],
  upstream: {
    connect_timeout_seconds: 10,
    response_header_timeout_seconds: 30,
    stream_idle_timeout_seconds: 90,
  },
  request_retry: {
    enabled: true,
    max_retries: 1,
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
  websocket: {
    enabled: false,
    max_idle_connections: 128,
    idle_timeout_seconds: 300,
    max_connection_age_seconds: 3_300,
  },
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const CONTROL_PLANE_USER: ControlPlaneUser = {
  id: ADMIN_USER.id,
  email: ADMIN_USER.email,
  display_name: ADMIN_USER.display_name,
  role: "admin",
  status: "active",
  can_reissue_invitation: false,
  password_change_required: false,
  temporary_password_expires_at: null,
  user_group_id: USER_GROUP.id,
  default_api_key_policy_id: API_KEY_POLICY.id,
  effective_api_key_policy_id: API_KEY_POLICY.id,
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

const PERSONAL_USAGE_COUNTS: Record<string, number> = {
  "2025-08-03": 1,
  "2025-08-04": 2,
  "2025-08-05": 3,
  "2025-08-06": 4,
  "2025-08-07": 5,
  "2025-08-08": 6,
  "2026-07-23": 3,
  "2026-07-24": 5,
  "2026-07-25": 8,
  "2026-07-26": 2,
  "2026-07-27": 4,
};

const PERSONAL_USAGE_DAYS: PersonalUsageReport["days"] = Array.from(
  { length: 365 },
  (_, index) => {
    const date = new Date(Date.UTC(2025, 6, 28 + index))
      .toISOString()
      .slice(0, 10);
    return {
      date,
      request_count: PERSONAL_USAGE_COUNTS[date] ?? 0,
    };
  },
);

export const PERSONAL_USAGE_REPORT: PersonalUsageReport = {
  started_on: "2025-07-28",
  ended_on: "2026-07-27",
  total_request_count: 43,
  active_day_count: 11,
  current_streak_days: 5,
  longest_streak_days: 6,
  days: PERSONAL_USAGE_DAYS,
};

export const COST_STATISTICS_REPORT: CostStatisticsReport = {
  started_at: "2026-07-14T14:00:00.000Z",
  ended_at: "2026-07-21T14:00:00.000Z",
  granularity: "day",
  summary: {
    request_count: 18_878,
    priced_request_count: 18_800,
    total_tokens: 263_000_000,
    input_tokens: 210_000_000,
    cached_input_tokens: 84_000_000,
    cache_write_tokens: 12_000_000,
    output_tokens: 53_000_000,
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
      input_tokens: 210_000_000,
      cached_input_tokens: 84_000_000,
      cache_write_tokens: 12_000_000,
      output_tokens: 53_000_000,
      success_rate: 0.975,
      cost_amount: "1912.06",
    },
  ],
  channels: [
    {
      id: CHANNEL.id,
      channel_group_id: CHANNEL_GROUP.id,
      channel_group_name: CHANNEL_GROUP.name,
      name: CHANNEL.name,
      api_format: CHANNEL.api_format,
      request_count: 18_878,
      total_tokens: 263_000_000,
      input_tokens: 210_000_000,
      cached_input_tokens: 84_000_000,
      cache_write_tokens: 12_000_000,
      output_tokens: 53_000_000,
      success_rate: 0.975,
      cost_amount: "1912.06",
    },
  ],
};

export const OWN_COST_STATISTICS_REPORT: CostStatisticsReport = {
  ...COST_STATISTICS_REPORT,
  channels: [],
};

export const SPEND_LEADERBOARD_REPORT: SpendLeaderboardReport = {
  period: "day",
  period_start: "2026-07-21",
  period_end: "2026-07-22",
  refreshed_at: "2026-07-21T14:15:00.000Z",
  total_cost_amount: "1912.06",
  previous_period_start: "2026-07-20",
  next_period_start: null,
  entries: [
    {
      rank: 1,
      user_id: "00000000-0000-0000-0000-000000000201",
      display_name: "Ada Lovelace",
      request_count: 6_810,
      priced_request_count: 6_800,
      total_tokens: 98_300_000,
      cost_amount: "721.95",
    },
    {
      rank: 2,
      user_id: "00000000-0000-0000-0000-000000000202",
      display_name: "Diego Rivera",
      request_count: 5_024,
      priced_request_count: 5_000,
      total_tokens: 74_200_000,
      cost_amount: "602.12",
    },
    {
      rank: 3,
      user_id: "00000000-0000-0000-0000-000000000203",
      display_name: "Lin Qiao",
      request_count: 3_106,
      priced_request_count: 3_085,
      total_tokens: 51_400_000,
      cost_amount: "385.12",
    },
    {
      rank: 4,
      user_id: "00000000-0000-0000-0000-000000000204",
      display_name: "Mary Jackson",
      request_count: 2_938,
      priced_request_count: 2_915,
      total_tokens: 39_100_000,
      cost_amount: "202.87",
    },
  ],
};

export const SESSION_AFFINITY_CACHE_REPORT: SessionAffinityCacheReport = {
  enabled: false,
  max_entries: 100_000,
  total_entries: 0,
  rules: [],
};

export const SYSTEM_LOAD_REPORT: SystemLoadReport = {
  sampled_at: "2026-07-22T10:00:00.000Z",
  started_at: "2026-07-22T08:00:00.000Z",
  uptime_seconds: 7_200,
  host: {
    logical_cpu_count: 8,
    cpu_usage_percent: 42.5,
    load_average_1m: 1.25,
    load_average_5m: 1.1,
    load_average_15m: 0.95,
    memory_total_bytes: 17_179_869_184,
    memory_used_bytes: 8_589_934_592,
    memory_usage_percent: 50,
  },
  process: {
    cpu_usage_percent: 12.5,
    resident_memory_bytes: 536_870_912,
    resident_memory_percent: 3.125,
    open_file_descriptors: 96,
    threads: 18,
  },
  runtime: {
    tracked_api_keys: 12,
    requests_in_current_windows: 320,
    in_flight_requests: 7,
    routing_in_flight_requests: 6,
    tracked_channels: 4,
    cooling_down_channels: 1,
    half_open_channels: 0,
    session_affinity_entries: 128,
  },
  queues: {
    request_log_notifications: {
      depth: 128,
      capacity: 1_024,
      utilization_percent: 12.5,
    },
    request_log_projection: {
      depth: 1,
      capacity: 1,
      utilization_percent: 100,
    },
    automatic_disable: {
      depth: 0,
      capacity: 1_024,
      utilization_percent: 0,
    },
  },
  request_log: {
    spool_pending_bytes: 2_097_152,
    ingress_backlog_rows_estimate: 240,
    ingress_oldest_age_seconds: 12,
    settlement_backlog_rows: 18,
    settlement_oldest_age_seconds: 4,
    recorded_total: 20_000,
    spooled_total: 20_000,
    projected_rows_total: 19_760,
    projection_deferred_total: 0,
    settled_rows_total: 19_742,
    spool_append_failures_total: 0,
    ingress_failures_total: 0,
    projection_failures_total: 0,
    settlement_failures_total: 0,
  },
  websocket: {
    enabled: true,
    active_downstream_sessions: 3,
    idle_upstream_connections: 12,
    leased_upstream_connections: 2,
    pool_capacity: 128,
    idle_pool_utilization_percent: 9.375,
    pool_hits_total: 900,
    pool_misses_total: 100,
    pool_discarded_total: 25,
    idle_timeout_seconds: 300,
    max_connection_age_seconds: 3_300,
  },
  image_body_spool: {
    active_files: 2,
    active_bytes: 67_108_864,
    available_bytes: 536_870_912_000,
    spooled_total: 48,
    spooled_bytes_total: 1_610_612_736,
    storage_failures_total: 0,
  },
  database: {
    control_plane: {
      size: 5,
      idle: 3,
      in_use: 2,
      capacity: 20,
      utilization_percent: 10,
    },
    request_log: {
      size: 4,
      idle: 2,
      in_use: 2,
      capacity: 4,
      utilization_percent: 50,
    },
  },
};
