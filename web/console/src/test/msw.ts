import { http, HttpResponse, passthrough } from "msw";
import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll } from "vitest";
import {
  ADMIN_ACCESS_TOKEN,
  ADMIN_API_KEY,
  ADMIN_LOGIN_RESPONSE,
  ADMIN_PROFILE,
  ADMIN_USER,
  ACTIVE_SESSION,
  API_KEY_OPTIONS,
  API_KEY_POLICY,
  CHANNEL,
  CHANNEL_DETAIL,
  CHANNEL_GROUP,
  CHANNEL_STATUS_REPORT,
  CONFIG_TEMPLATE,
  CONFIG_TEMPLATE_DETAIL,
  CONTROL_PLANE_USER,
  COST_STATISTICS_REPORT,
  DEFAULT_USER_GROUP,
  EXPIRED_SESSION,
  MODEL,
  MODEL_RULE,
  NEW_API_KEY_SECRET,
  OWN_CODEX_QUOTA,
  OWN_CODEX_QUOTA_HISTORY,
  OWN_COST_STATISTICS_REPORT,
  OTHER_ACTIVE_SESSION,
  OWN_API_KEY,
  PERSONAL_USAGE_REPORT,
  PROXY,
  PROXY_TEST_RESULT,
  REGISTRATION_INVITATION_CODE,
  REVOKED_SESSION,
  SESSION_AFFINITY_CACHE_REPORT,
  SPEND_LEADERBOARD_REPORT,
  SYSTEM_SETTINGS,
  SYSTEM_LOAD_REPORT,
  TEMPORARY_PASSWORD_ACCESS_TOKEN,
  TEMPORARY_PASSWORD_LOGIN_RESPONSE,
  USER_ACCESS_TOKEN,
  USER_GROUP,
  USER_SETTINGS,
  USER_USER,
} from "@/test/fixtures";
import { clearSession, setSession } from "@/api/session-store";

/**
 * Default MSW handlers covering the routes touched by the component tests.
 * Routes are registered relative to `location.origin` (jsdom defaults to
 * http://localhost), matching how the SPA fetches relative `/console/v1/*`
 * URLs. Override per-test with `server.use(...)` for mutations that return
 * secrets or change state.
 */
export const handlers = [
  http.post("/console/v1/auth/refresh", () =>
    HttpResponse.json(ADMIN_LOGIN_RESPONSE, { headers: { "Set-Cookie": "refresh=rotated" } }),
  ),
  http.post("/console/v1/auth/login", () =>
    HttpResponse.json(ADMIN_LOGIN_RESPONSE, { headers: { "Set-Cookie": "refresh=rotated" } }),
  ),
  http.post("/console/v1/auth/register", () =>
    HttpResponse.json(ADMIN_LOGIN_RESPONSE, { headers: { "Set-Cookie": "refresh=rotated" } }),
  ),
  http.post("/console/v1/auth/logout", () => new HttpResponse(null, { status: 204 })),
  http.post("/console/v1/auth/complete-password-reset", () =>
    HttpResponse.json(ADMIN_LOGIN_RESPONSE, {
      headers: { "Set-Cookie": "refresh=rotated" },
    }),
  ),

  http.get("/console/v1/me", () => HttpResponse.json(ADMIN_PROFILE)),
  http.patch("/console/v1/me", async ({ request }) => {
    const body = (await request.json()) as { display_name?: string };
    return HttpResponse.json({
      ...ADMIN_PROFILE,
      display_name: body.display_name ?? ADMIN_PROFILE.display_name,
    });
  }),
  http.get("/console/v1/me/settings", () => HttpResponse.json(USER_SETTINGS)),
  http.put("/console/v1/me/settings", async ({ request }) => {
    const body = (await request.json()) as { websocket_enabled?: boolean };
    return HttpResponse.json({
      ...USER_SETTINGS,
      websocket_enabled: body.websocket_enabled ?? USER_SETTINGS.websocket_enabled,
    });
  }),
  http.post("/console/v1/me/password", () => new HttpResponse(null, { status: 204 })),

  http.get("/console/v1/me/sessions", () =>
    HttpResponse.json([
      ACTIVE_SESSION,
      OTHER_ACTIVE_SESSION,
      REVOKED_SESSION,
      EXPIRED_SESSION,
    ]),
  ),
  http.delete("/console/v1/me/sessions", () => new HttpResponse(null, { status: 204 })),
  http.delete("/console/v1/me/sessions/:id", () => new HttpResponse(null, { status: 204 })),

  http.get("/console/v1/me/api-keys", () => HttpResponse.json([OWN_API_KEY])),
  http.get("/console/v1/me/codex-quotas", () =>
    HttpResponse.json([OWN_CODEX_QUOTA]),
  ),
  http.get("/console/v1/me/codex-quotas/:id/windows", () =>
    HttpResponse.json(OWN_CODEX_QUOTA_HISTORY),
  ),
  http.get("/console/v1/me/api-key-options", () => HttpResponse.json(API_KEY_OPTIONS)),
  http.get("/console/v1/me/api-hosts", () =>
    HttpResponse.json({ api_hosts: SYSTEM_SETTINGS.api_hosts }),
  ),
  http.get("/console/v1/me/api-keys/:id", () =>
    HttpResponse.json(OWN_API_KEY, {
      headers: { ETag: `"${OWN_API_KEY.updated_at}"` },
    }),
  ),
  http.post("/console/v1/me/api-keys", () =>
    HttpResponse.json(
      {
        id: OWN_API_KEY.id,
        secret: NEW_API_KEY_SECRET,
        correlation_id: "11111111-0000-0000-0000-000000000000",
      },
      { status: 201 },
    ),
  ),

  http.get("/console/v1/users", () => HttpResponse.json([CONTROL_PLANE_USER])),
  http.get("/console/v1/users/:id", () =>
    HttpResponse.json(CONTROL_PLANE_USER, {
      headers: { ETag: `"${CONTROL_PLANE_USER.updated_at}"` },
    }),
  ),
  http.post("/console/v1/users/:id/temporary-password", ({ params }) =>
    HttpResponse.json(
      {
        user_id: String(params.id),
        temporary_password: "AGW-test-temporary-password",
        expires_at: "2099-08-02T00:00:00.000Z",
        correlation_id: "11111111-0000-0000-0000-0000000000d1",
      },
      { status: 201 },
    ),
  ),
  http.get("/console/v1/user-groups", () =>
    HttpResponse.json([DEFAULT_USER_GROUP, USER_GROUP]),
  ),
  http.get("/console/v1/user-groups/:id", () =>
    HttpResponse.json(USER_GROUP, {
      headers: { ETag: `"${USER_GROUP.updated_at}"` },
    }),
  ),
  http.get("/console/v1/registration-invitation-codes", () =>
    HttpResponse.json([REGISTRATION_INVITATION_CODE]),
  ),
  http.get("/console/v1/registration-invitation-codes/:id", () =>
    HttpResponse.json(REGISTRATION_INVITATION_CODE, {
      headers: { ETag: `"${REGISTRATION_INVITATION_CODE.updated_at}"` },
    }),
  ),
  http.post("/console/v1/registration-invitation-codes", () =>
    HttpResponse.json(
      {
        id: REGISTRATION_INVITATION_CODE.id,
        invitation_code: "COMMUNITY-ACCESS-2026",
        correlation_id: "11111111-0000-0000-0000-0000000000c1",
      },
      { status: 201 },
    ),
  ),
  http.put("/console/v1/registration-invitation-codes/:id", () =>
    HttpResponse.json({
      id: REGISTRATION_INVITATION_CODE.id,
      correlation_id: "11111111-0000-0000-0000-0000000000c2",
    }),
  ),
  http.get("/console/v1/api-key-policies", () => HttpResponse.json([API_KEY_POLICY])),
  http.get("/console/v1/api-key-policies/:id", () =>
    HttpResponse.json(API_KEY_POLICY, {
      headers: { ETag: `"${API_KEY_POLICY.updated_at}"` },
    }),
  ),
  http.get("/console/v1/api-keys", () => HttpResponse.json([ADMIN_API_KEY])),
  http.get("/console/v1/models", () => HttpResponse.json([MODEL])),
  http.get("/console/v1/models/:id", () =>
    HttpResponse.json(MODEL, {
      headers: { ETag: `"${MODEL.updated_at}"` },
    }),
  ),
  http.get("/console/v1/routing/channel-groups", () => HttpResponse.json([CHANNEL_GROUP])),
  http.get("/console/v1/routing/channel-groups/:id", () =>
    HttpResponse.json(CHANNEL_GROUP, {
      headers: { ETag: `"${CHANNEL_GROUP.updated_at}"` },
    }),
  ),
  http.get("/console/v1/routing/channels", () => HttpResponse.json([CHANNEL])),
  http.get("/console/v1/routing/channels/:id", () =>
    HttpResponse.json(CHANNEL_DETAIL, {
      headers: { ETag: `"${CHANNEL.updated_at}"` },
    }),
  ),
  http.post("/console/v1/routing/channels/models/discover", () =>
    HttpResponse.json({ models: CHANNEL.available_models }),
  ),
  http.post("/console/v1/routing/channels/batch", () =>
    HttpResponse.json({
      updated_ids: [CHANNEL.id],
      correlation_id: "99999999-0000-0000-0000-000000000000",
    }),
  ),
  http.post("/console/v1/routing/channels/:id/recover", () =>
    HttpResponse.json({
      id: CHANNEL.id,
      correlation_id: "99999999-0000-0000-0000-000000000001",
    }),
  ),
  http.get("/console/v1/routing/model-rules", () => HttpResponse.json([MODEL_RULE])),
  http.get("/console/v1/routing/model-rules/:id", () =>
    HttpResponse.json(MODEL_RULE, {
      headers: { ETag: `"${MODEL_RULE.updated_at}"` },
    }),
  ),
  http.get("/console/v1/network/proxies", () => HttpResponse.json([PROXY])),
  http.get("/console/v1/network/proxies/:id", () =>
    HttpResponse.json(PROXY, {
      headers: { ETag: `"${PROXY.updated_at}"` },
    }),
  ),
  http.post("/console/v1/network/proxies/test", () =>
    HttpResponse.json(PROXY_TEST_RESULT),
  ),
  http.get("/console/v1/transforms/templates", () => HttpResponse.json([CONFIG_TEMPLATE])),
  http.get("/console/v1/transforms/templates/:id", () =>
    HttpResponse.json(CONFIG_TEMPLATE_DETAIL, {
      headers: { ETag: `"${CONFIG_TEMPLATE.updated_at}"` },
    }),
  ),
  http.get("/console/v1/request-logs", () => HttpResponse.json([])),
  http.get("/console/v1/me/request-logs", () => HttpResponse.json([])),
  http.get("/console/v1/me/usage", () => HttpResponse.json(PERSONAL_USAGE_REPORT)),
  http.get("/console/v1/statistics/channel-status", () =>
    HttpResponse.json(CHANNEL_STATUS_REPORT),
  ),
  http.get("/console/v1/statistics/costs", () =>
    HttpResponse.json(OWN_COST_STATISTICS_REPORT),
  ),
  http.get("/console/v1/system/statistics/costs", () =>
    HttpResponse.json(COST_STATISTICS_REPORT),
  ),
  http.get("/console/v1/statistics/spend-leaderboard", () =>
    HttpResponse.json(SPEND_LEADERBOARD_REPORT),
  ),
  http.get("/console/v1/system/load", () => HttpResponse.json(SYSTEM_LOAD_REPORT)),
  http.get("/console/v1/system/session-affinity/cache", () =>
    HttpResponse.json(SESSION_AFFINITY_CACHE_REPORT),
  ),
  http.delete("/console/v1/system/session-affinity/cache", () =>
    HttpResponse.json({
      cleared_entries: 0,
      cache: SESSION_AFFINITY_CACHE_REPORT,
    }),
  ),
  http.get("/console/v1/audit-logs", () => HttpResponse.json([])),
  http.get("/console/v1/system/settings", () =>
    HttpResponse.json(SYSTEM_SETTINGS, {
      headers: { ETag: `"${SYSTEM_SETTINGS.updated_at}"` },
    }),
  ),
  http.put("/console/v1/system/settings", () =>
    HttpResponse.json({
      id: "00000000-0000-0000-0000-0000000000f1",
      correlation_id: "11111111-0000-0000-0000-000000000000",
    }),
  ),

  // Anything not mocked falls through to the real network (fails loudly),
  // which keeps tests honest about coverage.
  http.all("*", () => passthrough()),
];

export const server = setupServer(...handlers);

beforeAll(() => server.listen({ onUnhandledRequest: "warn" }));
afterEach(() => {
  server.resetHandlers();
  clearSession();
});
afterAll(() => server.close());

/** Pre-seeds the in-memory session so a page renders as already authenticated. */
export function seedAuthenticatedSession() {
  setSession({
    status: "authenticated",
    accessToken: ADMIN_ACCESS_TOKEN,
    user: ADMIN_USER,
  });
}

/** Pre-seeds a regular-user session for role-sensitive page tests. */
export function seedUserSession() {
  server.use(
    http.post("/console/v1/auth/refresh", () =>
      HttpResponse.json({
        access_token: USER_ACCESS_TOKEN,
        token_type: "Bearer",
        expires_in: 900,
        user: USER_USER,
      }),
    ),
  );
  setSession({
    status: "authenticated",
    accessToken: USER_ACCESS_TOKEN,
    user: USER_USER,
  });
}

/** Pre-seeds a temporary-password session restricted to the reset flow. */
export function seedPasswordChangeSession() {
  server.use(
    http.post("/console/v1/auth/refresh", () =>
      HttpResponse.json(TEMPORARY_PASSWORD_LOGIN_RESPONSE),
    ),
  );
  setSession({
    status: "authenticated",
    accessToken: TEMPORARY_PASSWORD_ACCESS_TOKEN,
    user: TEMPORARY_PASSWORD_LOGIN_RESPONSE.user,
  });
}
