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
  MODEL,
  MODEL_RULE,
  NEW_API_KEY_SECRET,
  OWN_API_KEY,
  PROXY,
  REVOKED_SESSION,
  SYSTEM_SETTINGS,
  SYSTEM_LOAD_REPORT,
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
  http.post("/console/v1/auth/logout", () => new HttpResponse(null, { status: 204 })),

  http.get("/console/v1/me", () => HttpResponse.json(ADMIN_PROFILE)),
  http.patch("/console/v1/me", async ({ request }) => {
    const body = (await request.json()) as { display_name?: string };
    return HttpResponse.json({
      ...ADMIN_PROFILE,
      display_name: body.display_name ?? ADMIN_PROFILE.display_name,
    });
  }),
  http.post("/console/v1/me/password", () => new HttpResponse(null, { status: 204 })),

  http.get("/console/v1/me/sessions", () => HttpResponse.json([ACTIVE_SESSION, REVOKED_SESSION])),
  http.delete("/console/v1/me/sessions/:id", () => new HttpResponse(null, { status: 204 })),

  http.get("/console/v1/me/api-keys", () => HttpResponse.json([OWN_API_KEY])),
  http.get("/console/v1/me/api-key-options", () => HttpResponse.json(API_KEY_OPTIONS)),
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
  http.get("/console/v1/transforms/templates", () => HttpResponse.json([CONFIG_TEMPLATE])),
  http.get("/console/v1/transforms/templates/:id", () =>
    HttpResponse.json(CONFIG_TEMPLATE_DETAIL, {
      headers: { ETag: `"${CONFIG_TEMPLATE.updated_at}"` },
    }),
  ),
  http.get("/console/v1/request-logs", () => HttpResponse.json([])),
  http.get("/console/v1/me/request-logs", () => HttpResponse.json([])),
  http.get("/console/v1/statistics/channel-status", () =>
    HttpResponse.json(CHANNEL_STATUS_REPORT),
  ),
  http.get("/console/v1/statistics/costs", () =>
    HttpResponse.json(COST_STATISTICS_REPORT),
  ),
  http.get("/console/v1/system/load", () => HttpResponse.json(SYSTEM_LOAD_REPORT)),
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
