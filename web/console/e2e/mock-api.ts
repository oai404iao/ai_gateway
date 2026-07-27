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

export async function mockConsoleApi(page: Page): Promise<void> {
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
    if (path === "/console/v1/me/sessions" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
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
    if (path === "/console/v1/routing/channel-groups" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
    }
    if (path === "/console/v1/routing/channels" && method === "GET") {
      return route.fulfill({ status: 200, json: [] });
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
