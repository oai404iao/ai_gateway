import type { Page, Route } from "@playwright/test";

/**
 * Network-layer Console API mocks for e2e tests. Each handler returns
 * deterministic JSON so the SPA can be exercised in a real browser without
 * the Rust binary or PostgreSQL. Keep the shapes aligned with
 * `docs/openapi/console-v1.yaml` and `src/test/fixtures.ts`.
 */

const ADMIN_PROFILE = {
  user: {
    id: "00000000-0000-0000-0000-000000000001",
    email: "admin@example.com",
    display_name: "Initial Admin",
    role: "admin",
    password_change_required: false,
    temporary_password_expires_at: null,
    status: "active",
    default_api_key_policy_id: null,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
  },
  access_token: "e2e-mock-access-token",
};

const TEMPORARY_PASSWORD_PROFILE = {
  user: {
    id: "00000000-0000-0000-0000-000000000003",
    email: "reset@example.com",
    display_name: "Password Reset User",
    role: "user",
    password_change_required: true,
    temporary_password_expires_at: "2099-08-02T00:00:00.000Z",
    status: "active",
    balance_amount: "0.00",
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
  },
  access_token: "e2e-password-change-access-token",
};

const RESET_COMPLETED_PROFILE = {
  user: {
    ...TEMPORARY_PASSWORD_PROFILE.user,
    password_change_required: false,
    temporary_password_expires_at: null,
  },
  access_token: "e2e-reset-completed-access-token",
};

const E2E_USER = {
  id: "00000000-0000-0000-0000-000000000090",
  email: "batch-user@example.test",
  display_name: "Batch User",
  role: "user",
  status: "active",
  can_reissue_invitation: false,
  password_change_required: false,
  temporary_password_expires_at: null,
  user_group_id: "00000000-0000-0000-0000-000000000101",
  default_api_key_policy_id: null,
  effective_api_key_policy_id: "00000000-0000-0000-0000-000000000031",
  balance_amount: "10.00",
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

export const E2E_ADMIN_USER_GROUP_ID =
  "00000000-0000-0000-0000-000000000102";

const E2E_USER_GROUPS = [
  {
    id: "00000000-0000-0000-0000-000000000101",
    name: "Default Users",
    description: "Default group for newly invited users.",
    default_api_key_policy_id: "00000000-0000-0000-0000-000000000031",
    visible_codex_quota_group_ids: [],
    system_role: "user",
    member_count: 1,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-02T00:00:00.000Z",
  },
  {
    id: E2E_ADMIN_USER_GROUP_ID,
    name: "Default Administrators",
    description: "Default group for newly invited administrators.",
    default_api_key_policy_id: "00000000-0000-0000-0000-000000000031",
    visible_codex_quota_group_ids: [],
    system_role: "admin",
    member_count: 1,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-02T00:00:00.000Z",
  },
];

const E2E_API_KEY_POLICY = {
  id: "00000000-0000-0000-0000-000000000031",
  name: "default",
  allowed_group_ids: ["00000000-0000-0000-0000-000000000021"],
  allowed_channel_ids: [],
  enabled: true,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

const E2E_REGISTRATION_CODE = {
  id: "00000000-0000-0000-0000-0000000000c1",
  name: "Community launch",
  max_uses: 100,
  used_count: 12,
  expires_at: "2030-01-01T00:00:00.000Z",
  enabled: true,
  user_group_id: E2E_USER_GROUPS[0].id,
  initial_balance_amount: "20.00",
  created_by: ADMIN_PROFILE.user.id,
  last_used_at: "2026-07-26T12:00:00.000Z",
  created_at: "2026-07-01T00:00:00.000Z",
  updated_at: "2026-07-26T12:00:00.000Z",
};

const E2E_SESSIONS = [
  {
    id: "00000000-0000-0000-0000-0000000000e1",
    user_agent:
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15",
    created_at: "2026-07-27T08:00:00.000Z",
    last_seen_at: "2026-07-27T10:00:00.000Z",
    expires_at: "2099-08-26T08:00:00.000Z",
    revoked_at: null,
    state: "active",
    is_current: true,
  },
  {
    id: "00000000-0000-0000-0000-0000000000e2",
    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Firefox/128.0",
    created_at: "2026-07-26T08:00:00.000Z",
    last_seen_at: "2026-07-27T09:00:00.000Z",
    expires_at: "2099-08-25T08:00:00.000Z",
    revoked_at: null,
    state: "active",
    is_current: false,
  },
  {
    id: "00000000-0000-0000-0000-0000000000e3",
    user_agent: "curl/8.7.1 (Linux)",
    created_at: "2026-01-05T10:00:00.000Z",
    last_seen_at: "2026-01-05T10:00:00.000Z",
    expires_at: "2026-01-12T10:00:00.000Z",
    revoked_at: "2026-01-06T10:00:00.000Z",
    state: "revoked",
    is_current: false,
  },
];

export const E2E_API_KEY_SECRET = "sk-e2e-retrievable-api-key";
export const E2E_CODEX_GROUP_ID = "00000000-0000-0000-0000-00000000c001";
export const E2E_CODEX_CREDENTIAL_ID =
  "00000000-0000-0000-0000-00000000c002";

const E2E_CODEX_GROUP = {
  id: E2E_CODEX_GROUP_ID,
  name: "Codex subscriptions",
  api_format: "open_ai_responses",
  connector_kind: "codex_oauth",
  connector_pool_id: E2E_CODEX_GROUP_ID,
  priority: 0,
  selection_strategy: "weighted_random",
  enabled: true,
  updated_at: "2026-07-29T12:00:00.000Z",
};

const E2E_CODEX_IMAGES_GROUP = {
  ...E2E_CODEX_GROUP,
  id: "00000000-0000-0000-0000-00000000c010",
  name: "Codex subscriptions Images",
  api_format: "open_ai_images",
  enabled: false,
};

const E2E_STANDARD_CHANNEL_GROUPS = Array.from({ length: 5 }, (_, index) => ({
  id: `00000000-0000-0000-0000-0000000002${index}`,
  name: index === 4 ? "target-group" : `standard-group-${index + 1}`,
  api_format: "open_ai_chat_completions",
  connector_kind: "openai_compatible",
  connector_pool_id: null,
  priority: index,
  selection_strategy: "weighted_random",
  enabled: true,
  updated_at: "2026-07-29T12:00:00.000Z",
}));
export const E2E_STANDARD_GROUP_ID = E2E_STANDARD_CHANNEL_GROUPS[0].id;

const E2E_ROUTING_CHANNEL_GROUPS = [
  ...E2E_STANDARD_CHANNEL_GROUPS,
  E2E_CODEX_GROUP,
  E2E_CODEX_IMAGES_GROUP,
];

function routingChannel({
  id,
  channelGroupId,
  name,
  apiFormat = "open_ai_chat_completions",
  connectorKind = "openai_compatible",
  providerManaged = false,
}: {
  id: string;
  channelGroupId: string;
  name: string;
  apiFormat?: "open_ai_chat_completions" | "open_ai_responses" | "open_ai_images";
  connectorKind?: "openai_compatible" | "codex_oauth";
  providerManaged?: boolean;
}) {
  return {
    id,
    channel_group_id: channelGroupId,
    api_format: apiFormat,
    connector_kind: connectorKind,
    provider_managed: providerManaged,
    name,
    base_url: "https://upstream.e2e.example.test",
    enabled: true,
    supports_websocket: apiFormat === "open_ai_responses",
    status_statistics_enabled: !providerManaged,
    auto_disabled: false,
    auto_disabled_reason: null,
    auto_disable_allowed: !providerManaged,
    weight: 100,
    billing_multiplier: "1.00",
    proxy_id: null,
    config_template_id: null,
    connect_timeout_ms: null,
    response_header_timeout_ms: null,
    stream_idle_timeout_ms: null,
    upstream_auth_kind: providerManaged ? "none" : "bearer",
    upstream_auth_header_name: null,
    upstream_credential_configured: !providerManaged,
    available_models:
      apiFormat === "open_ai_images" ? ["gpt-image-2"] : ["gpt-5-codex"],
    test_model: apiFormat === "open_ai_images" ? null : "gpt-5-codex",
    created_at: "2026-07-29T12:00:00.000Z",
    updated_at: "2026-07-29T12:00:00.000Z",
  };
}

const E2E_ROUTING_CHANNELS = [
  ...E2E_STANDARD_CHANNEL_GROUPS.map((group, index) =>
    routingChannel({
      id: `00000000-0000-0000-0000-0000000003${index}`,
      channelGroupId: group.id,
      name: index === 4 ? "needle-upstream" : `standard-upstream-${index + 1}`,
    }),
  ),
  routingChannel({
    id: E2E_CODEX_CREDENTIAL_ID,
    channelGroupId: E2E_CODEX_GROUP_ID,
    name: "Personal Plus",
    apiFormat: "open_ai_responses",
    connectorKind: "codex_oauth",
    providerManaged: true,
  }),
  routingChannel({
    id: "00000000-0000-0000-0000-00000000c011",
    channelGroupId: E2E_CODEX_IMAGES_GROUP.id,
    name: "Personal Plus",
    apiFormat: "open_ai_images",
    connectorKind: "codex_oauth",
    providerManaged: true,
  }),
];

export const E2E_CODEX_CREDENTIAL = {
  id: E2E_CODEX_CREDENTIAL_ID,
  channel_group_id: E2E_CODEX_GROUP_ID,
  label: "Personal Plus",
  email: "codex@example.test",
  account_id: "account-123",
  user_id: "user-123",
  plan_type: "plus",
  is_fedramp: false,
  access_token_expires_at: "2026-07-29T15:00:00.000Z",
  last_refreshed_at: "2026-07-29T12:00:00.000Z",
  quota_threshold_percent: 95,
  runtime_status: "draining",
  quota_allowed: true,
  quota_limit_reached: false,
  primary_used_percent: 96,
  primary_window_seconds: 10_800,
  primary_reset_at: "2026-07-29T15:00:00.000Z",
  secondary_used_percent: null,
  secondary_window_seconds: null,
  secondary_reset_at: null,
  quota_reset_credits_available: 2,
  quota_checked_at: "2026-07-29T12:00:00.000Z",
  last_error_code: null,
  last_error_summary: null,
  proxy_id: null,
  weight: 100,
  enabled: true,
  available_models: ["gpt-5-codex"],
  created_at: "2026-07-29T12:00:00.000Z",
  updated_at: "2026-07-29T12:00:00.000Z",
};

const E2E_COST_STATISTICS = {
  started_at: "2026-07-01T00:00:00.000Z",
  ended_at: "2026-07-24T12:00:00.000Z",
  granularity: "day",
  summary: {
    request_count: 12,
    priced_request_count: 12,
    total_tokens: 1_200_000,
    input_tokens: 900_000,
    cached_input_tokens: 300_000,
    cache_write_tokens: 20_000,
    output_tokens: 300_000,
    average_rpm: 0.5,
    average_tpm: 50_000,
    cost_amount: "1912.06",
  },
  buckets: [],
  models: [],
  channels: [],
};

const E2E_PERSONAL_USAGE_COUNTS: Record<string, number> = {
  "2026-07-24": 2,
  "2026-07-25": 4,
  "2026-07-26": 1,
  "2026-07-27": 6,
};

const E2E_PERSONAL_USAGE = {
  started_on: "2025-07-28",
  ended_on: "2026-07-27",
  total_request_count: 13,
  active_day_count: 4,
  current_streak_days: 4,
  longest_streak_days: 4,
  days: Array.from({ length: 365 }, (_, index) => {
    const date = new Date(Date.UTC(2025, 6, 28 + index))
      .toISOString()
      .slice(0, 10);
    return {
      date,
      request_count: E2E_PERSONAL_USAGE_COUNTS[date] ?? 0,
    };
  }),
};

const E2E_SYSTEM_LOAD = {
  sampled_at: "2026-07-27T10:00:00.000Z",
  started_at: "2026-07-27T08:00:00.000Z",
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

const E2E_PROXY_TEST_RESULT = {
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
  isp: "E2E ISP",
  organization: "E2E Organization",
  autonomous_system: "AS64500 E2E",
  autonomous_system_name: "E2E",
  mobile: false,
  proxy: true,
  hosting: false,
  latency_ms: 42,
  rate_limit_remaining: 44,
  rate_limit_reset_seconds: 60,
};

const E2E_SPEND_LEADERBOARD = {
  period: "day",
  period_start: "2026-07-24",
  period_end: "2026-07-25",
  refreshed_at: "2026-07-24T12:15:00.000Z",
  total_cost_amount: "1912.06",
  previous_period_start: "2026-07-23",
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
  ],
};

const E2E_PERSONAL_REQUEST_LOG = {
  id: "00000000-0000-0000-0000-0000000000f1",
  started_at: "2026-07-28T10:00:00.000Z",
  completed_at: "2026-07-28T10:00:01.000Z",
  user_id: ADMIN_PROFILE.user.id,
  user_name: null,
  api_key_id: "00000000-0000-0000-0000-000000000011",
  request_source: "client",
  api_format: "open_ai_responses",
  api_operation: "responses",
  request_protocol: "sse",
  client_model: "gateway-e2e-model",
  reasoning_effort: "high",
  fast_mode: true,
  upstream_model: "upstream-e2e-model",
  model_rule_id: "00000000-0000-0000-0000-0000000000f2",
  channel_group_id: "00000000-0000-0000-0000-000000000021",
  channel_group_name: "chat-primary",
  channel_id: null,
  channel_name: null,
  outcome: "failed",
  response_status_code: 429,
  streamed: true,
  ttft_ms: 120,
  total_duration_ms: 1_000,
  output_tokens_per_second: "4.5455",
  input_tokens: 12,
  cached_input_tokens: 2,
  cache_write_tokens: 0,
  output_tokens: 4,
  reasoning_tokens: 1,
  cost_amount: "0.00010000",
  error_code: "rate_limit_exceeded",
  error_summary: "Upstream rate limit exceeded.",
  billed_at: "2026-07-28T10:00:02.000Z",
};

const E2E_SYSTEM_REQUEST_LOG = {
  ...E2E_PERSONAL_REQUEST_LOG,
  user_id: E2E_USER.id,
  user_name: E2E_USER.display_name,
  channel_id: "00000000-0000-0000-0000-000000000022",
  channel_name: "upstream-a",
};

export async function mockConsoleApi(page: Page): Promise<void> {
  let websocketEnabled = false;
  let authenticated = false;
  let session = ADMIN_PROFILE;
  await page.route("**/console/v1/**", (route: Route) => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const path = url.pathname.replace(/^.*console\/v1/, "/console/v1");

    if (path === "/console/v1/auth/login" && method === "POST") {
      // The SPA stores the access token from the response body. This in-memory
      // flag stands in for the HttpOnly refresh cookie after a full reload.
      authenticated = true;
      const input = route.request().postDataJSON() as { email?: string };
      session =
        input.email === TEMPORARY_PASSWORD_PROFILE.user.email
          ? TEMPORARY_PASSWORD_PROFILE
          : ADMIN_PROFILE;
      return route.fulfill({ status: 200, json: session });
    }
    if (path === "/console/v1/auth/register" && method === "POST") {
      authenticated = true;
      return route.fulfill({ status: 200, json: ADMIN_PROFILE });
    }
    if (path === "/console/v1/auth/refresh" && method === "POST") {
      if (authenticated) {
        return route.fulfill({ status: 200, json: session });
      }
      // Start unauthenticated so the login page renders; the login or
      // registration POST above establishes the mocked refresh session.
      return route.fulfill({
        status: 401,
        json: { error: "Unauthorized" },
      });
    }
    if (
      path === "/console/v1/auth/complete-password-reset" &&
      method === "POST"
    ) {
      session = RESET_COMPLETED_PROFILE;
      authenticated = true;
      return route.fulfill({ status: 200, json: session });
    }
    if (path === "/console/v1/me" && method === "GET") {
      return route.fulfill({ status: 200, json: session.user });
    }
    if (path === "/console/v1/me/settings" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: {
          websocket_enabled: websocketEnabled,
          updated_at: "2026-07-28T00:00:00.000Z",
        },
      });
    }
    if (path === "/console/v1/me/settings" && method === "PUT") {
      const input = route.request().postDataJSON() as {
        websocket_enabled: boolean;
      };
      websocketEnabled = input.websocket_enabled;
      return route.fulfill({
        status: 200,
        json: {
          websocket_enabled: websocketEnabled,
          updated_at: "2026-07-28T00:00:00.000Z",
        },
      });
    }
    if (path === "/console/v1/me/sessions" && method === "GET") {
      return route.fulfill({ status: 200, json: E2E_SESSIONS });
    }
    if (path === "/console/v1/me/sessions" && method === "DELETE") {
      return route.fulfill({ status: 204 });
    }
    if (path.startsWith("/console/v1/me/sessions/") && method === "DELETE") {
      return route.fulfill({ status: 204 });
    }
    if (path === "/console/v1/me/usage" && method === "GET") {
      return route.fulfill({ status: 200, json: E2E_PERSONAL_USAGE });
    }
    if (path === "/console/v1/me/codex-quotas" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: [
          {
            id: E2E_CODEX_CREDENTIAL_ID,
            name: E2E_CODEX_CREDENTIAL_ID,
            channel_group_id: E2E_CODEX_GROUP_ID,
            plan_type: "plus",
            primary_used_percent: 96,
            primary_window_seconds: 10_800,
            primary_reset_at: "2026-08-03T15:00:00.000Z",
            secondary_used_percent: 12,
            secondary_window_seconds: 604_800,
            secondary_reset_at: "2026-08-10T12:00:00.000Z",
            quota_checked_at: "2026-08-03T12:00:00.000Z",
          },
        ],
      });
    }
    if (
      path ===
        `/console/v1/me/codex-quotas/${E2E_CODEX_CREDENTIAL_ID}/windows` &&
      method === "GET"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          credential_id: E2E_CODEX_CREDENTIAL_ID,
          name: E2E_CODEX_CREDENTIAL_ID,
          channel_group_id: E2E_CODEX_GROUP_ID,
          plan_type: "plus",
          periods: [
            {
              window_kind: "primary",
              window_seconds: 10_800,
              started_at: "2026-08-03T09:00:00.000Z",
              scheduled_reset_at: "2026-08-03T12:00:00.000Z",
              ended_at: "2026-08-03T12:00:00.000Z",
              reset_reason: "natural",
              initial_used_percent: 5,
              last_used_percent: 96,
              first_observed_at: "2026-08-03T09:01:00.000Z",
              last_observed_at: "2026-08-03T11:59:00.000Z",
            },
          ],
        },
      });
    }
    if (path === "/console/v1/me/request-logs" && method === "GET") {
      return route.fulfill({ status: 200, json: [E2E_PERSONAL_REQUEST_LOG] });
    }
    if (
      path === `/console/v1/me/request-logs/${E2E_PERSONAL_REQUEST_LOG.id}` &&
      method === "GET"
    ) {
      return route.fulfill({ status: 200, json: E2E_PERSONAL_REQUEST_LOG });
    }
    if (path === "/console/v1/users" && method === "GET") {
      return route.fulfill({ status: 200, json: [E2E_USER] });
    }
    if (path === "/console/v1/api-keys" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/user-groups" && method === "GET") {
      return route.fulfill({ status: 200, json: E2E_USER_GROUPS });
    }
    if (
      path === `/console/v1/user-groups/${E2E_USER_GROUPS[1].id}` &&
      method === "GET"
    ) {
      return route.fulfill({
        status: 200,
        headers: { ETag: `"${E2E_USER_GROUPS[1].updated_at}"` },
        json: E2E_USER_GROUPS[1],
      });
    }
    if (
      path === `/console/v1/user-groups/${E2E_USER_GROUPS[1].id}` &&
      method === "PUT"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          id: E2E_USER_GROUPS[1].id,
          correlation_id: "00000000-0000-0000-0000-000000000103",
        },
      });
    }
    if (
      path === "/console/v1/registration-invitation-codes" &&
      method === "GET"
    ) {
      return route.fulfill({ status: 200, json: [E2E_REGISTRATION_CODE] });
    }
    if (
      path === "/console/v1/registration-invitation-codes" &&
      method === "POST"
    ) {
      return route.fulfill({
        status: 201,
        json: {
          id: E2E_REGISTRATION_CODE.id,
          invitation_code: "COMMUNITY-ACCESS-2026",
          correlation_id: "00000000-0000-0000-0000-0000000000c2",
        },
      });
    }
    if (
      path ===
        `/console/v1/registration-invitation-codes/${E2E_REGISTRATION_CODE.id}` &&
      method === "GET"
    ) {
      return route.fulfill({
        status: 200,
        headers: { ETag: `"${E2E_REGISTRATION_CODE.updated_at}"` },
        json: E2E_REGISTRATION_CODE,
      });
    }
    if (
      path ===
        `/console/v1/registration-invitation-codes/${E2E_REGISTRATION_CODE.id}` &&
      method === "PUT"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          id: E2E_REGISTRATION_CODE.id,
          correlation_id: "00000000-0000-0000-0000-0000000000c3",
        },
      });
    }
    if (path === "/console/v1/api-key-policies" && method === "GET") {
      return route.fulfill({ status: 200, json: [E2E_API_KEY_POLICY] });
    }
    if (path === "/console/v1/models" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/network/proxies" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/transforms/templates" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/users/batch" && method === "POST") {
      return route.fulfill({
        status: 200,
        json: {
          updated_ids: [E2E_USER.id],
          correlation_id: "00000000-0000-0000-0000-000000000091",
        },
      });
    }
    if (path === "/console/v1/me/api-keys" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: [
          {
            id: "00000000-0000-0000-0000-000000000011",
            name: "e2e key",
            secret: E2E_API_KEY_SECRET,
            status: "active",
            expires_at: null,
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
          },
        ],
      });
    }
    if (path === "/console/v1/me/api-hosts" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: { api_hosts: ["https://api.e2e.example.test/v1"] },
      });
    }
    if (path === "/console/v1/statistics/channel-status" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: {
          window: "24h",
          started_at: "2026-07-23T00:00:00.000Z",
          ended_at: "2026-07-24T00:00:00.000Z",
          bucket_seconds: 3600,
          models: [],
          channels: [],
        },
      });
    }
    if (path === "/console/v1/statistics/costs" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: { ...E2E_COST_STATISTICS, channels: [] },
      });
    }
    if (
      path === "/console/v1/system/statistics/costs" &&
      method === "GET"
    ) {
      return route.fulfill({ status: 200, json: E2E_COST_STATISTICS });
    }
    if (
      path === "/console/v1/statistics/spend-leaderboard" &&
      method === "GET"
    ) {
      return route.fulfill({ status: 200, json: E2E_SPEND_LEADERBOARD });
    }
    if (path === "/console/v1/system/load" && method === "GET") {
      return route.fulfill({ status: 200, json: E2E_SYSTEM_LOAD });
    }
    if (
      path === `/console/v1/routing/channel-groups/${E2E_CODEX_GROUP_ID}` &&
      method === "GET"
    ) {
      return route.fulfill({
        status: 200,
        headers: { ETag: `"${E2E_CODEX_GROUP.updated_at}"` },
        json: E2E_CODEX_GROUP,
      });
    }
    if (
      path ===
        `/console/v1/providers/codex-oauth/channel-groups/${E2E_CODEX_GROUP_ID}/credentials` &&
      method === "GET"
    ) {
      return route.fulfill({
        status: 200,
        json: [E2E_CODEX_CREDENTIAL],
      });
    }
    if (
      path ===
        `/console/v1/providers/codex-oauth/credentials/${E2E_CODEX_CREDENTIAL_ID}/quota/refresh` &&
      method === "POST"
    ) {
      return route.fulfill({ status: 204 });
    }
    if (
      path ===
        `/console/v1/providers/codex-oauth/credentials/${E2E_CODEX_CREDENTIAL_ID}/quota/reset` &&
      method === "POST"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          outcome: "reset",
          windows_reset: 2,
          quota_refreshed: true,
          correlation_id: "00000000-0000-0000-0000-00000000c004",
        },
      });
    }
    if (
      path ===
        `/console/v1/providers/codex-oauth/credentials/${E2E_CODEX_CREDENTIAL_ID}/quota/windows` &&
      method === "GET"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          credential_id: E2E_CODEX_CREDENTIAL_ID,
          periods: [
            {
              id: "00000000-0000-0000-0000-00000000c005",
              credential_id: E2E_CODEX_CREDENTIAL_ID,
              window_kind: "primary",
              window_seconds: 10_800,
              started_at: "2026-07-29T09:00:00.000Z",
              scheduled_reset_at: "2026-07-29T12:00:00.000Z",
              ended_at: "2026-07-29T12:00:00.000Z",
              reset_reason: "manual",
              initial_used_percent: 10,
              last_used_percent: 96,
              first_observed_at: "2026-07-29T09:01:00.000Z",
              last_observed_at: "2026-07-29T11:59:00.000Z",
            },
          ],
        },
      });
    }
    if (
      path ===
        `/console/v1/providers/codex-oauth/channel-groups/${E2E_CODEX_GROUP_ID}/credentials/batch` &&
      method === "POST"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          updated_ids: [E2E_CODEX_CREDENTIAL_ID],
          correlation_id: "00000000-0000-0000-0000-00000000c003",
        },
      });
    }
    if (path === "/console/v1/routing/channel-groups" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: E2E_ROUTING_CHANNEL_GROUPS,
      });
    }
    if (
      path.startsWith("/console/v1/routing/channel-groups/") &&
      method === "PUT"
    ) {
      return route.fulfill({
        status: 200,
        json: {
          id: path.split("/").at(-1),
          correlation_id: "00000000-0000-0000-0000-0000000002ff",
        },
      });
    }
    if (path === "/console/v1/routing/channels" && method === "GET") {
      return route.fulfill({ status: 200, json: E2E_ROUTING_CHANNELS });
    }
    if (path === "/console/v1/routing/model-rules" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/network/proxies/test" && method === "POST") {
      return route.fulfill({ status: 200, json: E2E_PROXY_TEST_RESULT });
    }
    if (path === "/console/v1/request-logs" && method === "GET") {
      return route.fulfill({ status: 200, json: [E2E_SYSTEM_REQUEST_LOG] });
    }
    if (
      path === `/console/v1/request-logs/${E2E_SYSTEM_REQUEST_LOG.id}` &&
      method === "GET"
    ) {
      return route.fulfill({ status: 200, json: E2E_SYSTEM_REQUEST_LOG });
    }
    if (path === "/console/v1/me/api-key-options" && method === "GET") {
      return route.fulfill({
        status: 200,
        json: {
          policy_id: "00000000-0000-0000-0000-000000000031",
          policy_name: "default",
          groups: [
            {
              id: "00000000-0000-0000-0000-000000000021",
              name: "chat-primary",
              api_format: "open_ai_chat_completions",
              priority: 1,
              enabled: true,
            },
            {
              id: "00000000-0000-0000-0000-000000000025",
              name: "images-disabled",
              api_format: "open_ai_images",
              priority: 1,
              enabled: false,
            },
          ],
          channels: [
            {
              id: "00000000-0000-0000-0000-000000000022",
              channel_group_id: "00000000-0000-0000-0000-000000000021",
              channel_group_name: "chat-primary",
              channel_group_enabled: true,
              api_format: "open_ai_chat_completions",
              name: "upstream-a",
              enabled: true,
              auto_disabled: false,
            },
            {
              id: "00000000-0000-0000-0000-000000000026",
              channel_group_id: "00000000-0000-0000-0000-000000000025",
              channel_group_name: "images-disabled",
              channel_group_enabled: false,
              api_format: "open_ai_images",
              name: "images-disabled-upstream",
              enabled: true,
              auto_disabled: false,
            },
          ],
        },
      });
    }
    if (path === "/console/v1/me/api-keys" && method === "POST") {
      return route.fulfill({
        status: 201,
        json: {
          id: "00000000-0000-0000-0000-000000000012",
          secret: "sk-e2e-created-api-key",
          correlation_id: "11111111-0000-0000-0000-000000000000",
        },
      });
    }
    // Default: empty 200 so unknown reads do not break the shell.
    return route.fulfill({ status: 200, json: {} });
  });
}
