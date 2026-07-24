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

export const E2E_API_KEY_SECRET = "sk-e2e-retrievable-api-key";

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
