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
    status: "active",
    default_api_key_policy_id: null,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
  },
  access_token: "e2e-mock-access-token",
};

const E2E_USER = {
  id: "00000000-0000-0000-0000-000000000090",
  email: "batch-user@example.test",
  display_name: "Batch User",
  role: "user",
  status: "active",
  can_reissue_invitation: false,
  user_group_id: "00000000-0000-0000-0000-000000000101",
  default_api_key_policy_id: null,
  effective_api_key_policy_id: "00000000-0000-0000-0000-000000000031",
  balance_amount: "10.00",
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-02T00:00:00.000Z",
};

const E2E_USER_GROUPS = [
  {
    id: "00000000-0000-0000-0000-000000000101",
    name: "Default Users",
    description: "Default group for newly invited users.",
    default_api_key_policy_id: "00000000-0000-0000-0000-000000000031",
    system_role: "user",
    member_count: 1,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-02T00:00:00.000Z",
  },
  {
    id: "00000000-0000-0000-0000-000000000102",
    name: "Default Administrators",
    description: "Default group for newly invited administrators.",
    default_api_key_policy_id: "00000000-0000-0000-0000-000000000031",
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
  request_protocol: "sse",
  client_model: "gateway-e2e-model",
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
  await page.route("**/console/v1/**", (route: Route) => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const path = url.pathname.replace(/^.*console\/v1/, "/console/v1");

    if (path === "/console/v1/auth/login" && method === "POST") {
      // The SPA stores the access_token from the response body; the HttpOnly
      // refresh cookie is irrelevant here because we fulfill the refresh route
      // below unconditionally.
      return route.fulfill({ status: 200, json: ADMIN_PROFILE });
    }
    if (path === "/console/v1/auth/register" && method === "POST") {
      return route.fulfill({ status: 200, json: ADMIN_PROFILE });
    }
    if (path === "/console/v1/auth/refresh" && method === "POST") {
      // Start unauthenticated so the login page renders; the login POST
      // below establishes the session.
      return route.fulfill({
        status: 401,
        json: { error: "Unauthorized" },
      });
    }
    if (path === "/console/v1/me" && method === "GET") {
      return route.fulfill({ status: 200, json: ADMIN_PROFILE.user });
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
    if (path === "/console/v1/routing/channel-groups" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/routing/channels" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/routing/model-rules" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
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
              enabled: true,
            },
          ],
          channels: [
            {
              id: "00000000-0000-0000-0000-000000000022",
              channel_group_id: "00000000-0000-0000-0000-000000000021",
              channel_group_name: "chat-primary",
              api_format: "open_ai_chat_completions",
              name: "upstream-a",
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
