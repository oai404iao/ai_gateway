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
      return route.fulfill({ status: 200, json: { data: [], etag: '""' } });
    }
    // Default: empty 200 so unknown reads do not break the shell.
    return route.fulfill({ status: 200, json: {} });
  });
}
