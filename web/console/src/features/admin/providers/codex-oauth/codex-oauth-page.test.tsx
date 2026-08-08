import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type {
  ChannelGroupView,
  CodexCredentialBatchInput,
  CodexCredentialExportInput,
  CodexCredentialView,
  CodexOauthCompleteInput,
  CodexOauthStartInput,
} from "@/api/types";
import { CHANNEL_GROUP } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

const GROUP_ID = "00000000-0000-0000-0000-00000000c001";
const CREDENTIAL_ID = "00000000-0000-0000-0000-00000000c002";
const FLOW_ID = "00000000-0000-0000-0000-00000000c003";

const CODEX_GROUP: ChannelGroupView = {
  ...CHANNEL_GROUP,
  id: GROUP_ID,
  name: "Codex subscriptions",
  api_format: "open_ai_responses",
  connector_kind: "codex_oauth",
  connector_pool_id: GROUP_ID,
};

const CREDENTIAL: CodexCredentialView = {
  id: CREDENTIAL_ID,
  channel_group_id: GROUP_ID,
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

function renderPage() {
  window.history.replaceState({}, "", `/admin/providers/codex-oauth/${GROUP_ID}`);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

function baseHandlers(credentials: CodexCredentialView[]) {
  return [
    http.get("/console/v1/routing/channel-groups/:id", () =>
      HttpResponse.json(CODEX_GROUP, {
        headers: { ETag: `"${CODEX_GROUP.updated_at}"` },
      }),
    ),
    http.get(
      "/console/v1/providers/codex-oauth/channel-groups/:id/credentials",
      () => HttpResponse.json(credentials),
    ),
  ];
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("CodexOauthPage", () => {
  it("returns to the channels page", async () => {
    seedAuthenticatedSession();
    server.use(...baseHandlers([]));
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: "Back to channels" }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/routing/channels");
    });
  });

  it("shows quota state and manually refreshes a credential quota", async () => {
    seedAuthenticatedSession();
    let refreshedId: string | undefined;
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.post(
        "/console/v1/providers/codex-oauth/credentials/:id/quota/refresh",
        ({ params }) => {
          refreshedId = String(params.id);
          return new HttpResponse(null, { status: 204 });
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    expect(await screen.findByText("Personal Plus")).toBeInTheDocument();
    expect(screen.getByText("96% used")).toBeInTheDocument();
    expect(screen.getByText("Draining")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: "Refresh quota for Personal Plus",
      }),
    );
    await waitFor(() => expect(refreshedId).toBe(CREDENTIAL_ID));
  });

  it("renders personal credentials without a workspace account id", async () => {
    seedAuthenticatedSession();
    server.use(
      ...baseHandlers([
        {
          ...CREDENTIAL,
          email: "free@example.test",
          account_id: null,
          plan_type: "free",
        },
      ]),
    );
    renderPage();

    expect(await screen.findByText("Personal Plus")).toBeInTheDocument();
    expect(
      screen.getByText("Personal credential (no workspace ID)"),
    ).toBeInTheDocument();
    expect(screen.queryByText("Workspace account-123")).not.toBeInTheDocument();
  });

  it("consumes an OpenAI reset credit with confirmation", async () => {
    seedAuthenticatedSession();
    let resetId: string | undefined;
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.post(
        "/console/v1/providers/codex-oauth/credentials/:id/quota/reset",
        ({ params }) => {
          resetId = String(params.id);
          return HttpResponse.json({
            outcome: "reset",
            windows_reset: 2,
            quota_refreshed: true,
            correlation_id: "00000000-0000-0000-0000-00000000c010",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", {
        name: "Reset quota with an OpenAI credit for Personal Plus",
      }),
    );
    expect(
      screen.getByRole("heading", {
        name: "Consume an OpenAI reset credit?",
      }),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Consume reset credit" }),
    );

    await waitFor(() => expect(resetId).toBe(CREDENTIAL_ID));
    expect(
      await screen.findByText("OpenAI reset credit consumed. 2 windows reset."),
    ).toBeInTheDocument();
  });

  it("opens quota-window history and jumps to system costs for both projections", async () => {
    seedAuthenticatedSession();
    const costQueries: URLSearchParams[] = [];
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.get(
        "/console/v1/providers/codex-oauth/credentials/:id/quota/windows",
        () =>
          HttpResponse.json({
            credential_id: CREDENTIAL_ID,
            periods: [
              {
                id: "00000000-0000-0000-0000-00000000c011",
                credential_id: CREDENTIAL_ID,
                window_kind: "primary",
                window_seconds: 10_800,
                started_at: "2026-07-29T09:00:17.000Z",
                scheduled_reset_at: "2026-07-29T12:00:17.000Z",
                ended_at: "2026-07-29T11:50:43.000Z",
                reset_reason: "manual",
                initial_used_percent: 12,
                last_used_percent: 96,
                first_observed_at: "2026-07-29T09:05:00.000Z",
                last_observed_at: "2026-07-29T11:49:00.000Z",
              },
              {
                id: "00000000-0000-0000-0000-00000000c012",
                credential_id: CREDENTIAL_ID,
                window_kind: "secondary",
                window_seconds: 604_800,
                started_at: "2026-07-22T12:00:00.000Z",
                scheduled_reset_at: "2026-07-29T12:00:00.000Z",
                ended_at: "2026-07-29T12:00:00.000Z",
                reset_reason: "openai_official",
                initial_used_percent: 1,
                last_used_percent: 70,
                first_observed_at: "2026-07-22T12:05:00.000Z",
                last_observed_at: "2026-07-29T11:55:00.000Z",
              },
            ],
          }),
      ),
      http.get("/console/v1/system/statistics/costs", ({ request }) => {
        costQueries.push(new URL(request.url).searchParams);
        return HttpResponse.json({
          started_at: "2026-07-29T09:00:00.000Z",
          ended_at: "2026-07-29T12:00:00.000Z",
          granularity: "hour",
          summary: {
            request_count: 0,
            priced_request_count: 0,
            total_tokens: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
            average_rpm: 0,
            average_tpm: 0,
            cost_amount: "0",
          },
          buckets: [],
          models: [],
          channels: [],
        });
      }),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", {
        name: "View quota history for Personal Plus",
      }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "Quota window history for Personal Plus",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Manual reset credit")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "View costs" }));

    await waitFor(() =>
      expect(window.location.pathname).toBe("/admin/cost-statistics"),
    );
    await waitFor(() => expect(costQueries.length).toBeGreaterThan(0));
    const query = costQueries.at(-1);
    expect(query?.get("started_after")).toBe("2026-07-29T09:00:17.000Z");
    expect(query?.get("started_before")).toBe("2026-07-29T11:50:43.000Z");
    expect(query?.get("codex_credential_id")).toBe(CREDENTIAL_ID);
    expect(query?.has("channel_id")).toBe(false);
  });

  it("exposes tooltip descriptions for credential actions", async () => {
    seedAuthenticatedSession();
    server.use(...baseHandlers([CREDENTIAL]));
    renderPage();

    expect(await screen.findByText("Personal Plus")).toBeInTheDocument();

    for (const label of [
      "View quota history for Personal Plus",
      "Edit Personal Plus",
      "Reset quota with an OpenAI credit for Personal Plus",
      "Refresh quota for Personal Plus",
      "Refresh token for Personal Plus",
      "Delete Personal Plus",
    ]) {
      const button = screen.getByRole("button", { name: label });
      fireEvent.focus(button);
      expect(
        await screen.findByText(label, {
          selector: '[data-slot="tooltip-content"]',
        }),
      ).toBeVisible();
      fireEvent.blur(button);
      await waitFor(() =>
        expect(
          screen.queryByText(label, {
            selector: '[data-slot="tooltip-content"]',
          }),
        ).not.toBeInTheDocument(),
      );
    }
  });

  it("starts PKCE authorization without opening a tab and submits the copied callback URL", async () => {
    seedAuthenticatedSession();
    const authorizationUrl =
      "https://auth.openai.com/oauth/authorize?client_id=client&state=state-value";
    let startInput: CodexOauthStartInput | undefined;
    let completeInput: CodexOauthCompleteInput | undefined;
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    server.use(
      ...baseHandlers([]),
      http.post(
        "/console/v1/providers/codex-oauth/channel-groups/:id/oauth/flows",
        async ({ request }) => {
          startInput = (await request.json()) as CodexOauthStartInput;
          return HttpResponse.json(
            {
              flow_id: FLOW_ID,
              authorization_url: authorizationUrl,
              expires_at: "2026-07-29T13:15:00.000Z",
            },
            { status: 201 },
          );
        },
      ),
      http.post(
        "/console/v1/providers/codex-oauth/oauth/flows/:id/complete",
        async ({ request }) => {
          completeInput = (await request.json()) as CodexOauthCompleteInput;
          return HttpResponse.json(
            {
              id: CREDENTIAL_ID,
              correlation_id: "00000000-0000-0000-0000-00000000c004",
            },
            { status: 201 },
          );
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("button", { name: "Connect account" }));
    await user.type(screen.getByLabelText("Label"), "Personal Plus");
    await user.click(screen.getByRole("button", { name: "Start authorization" }));

    await waitFor(() => expect(startInput?.label).toBe("Personal Plus"));
    expect(startInput).toMatchObject({
      proxy_id: null,
      weight: 100,
      quota_threshold_percent: 95,
    });
    expect(open).not.toHaveBeenCalled();

    await user.click(
      await screen.findByRole("button", { name: "Open authorization page" }),
    );
    expect(open).toHaveBeenCalledWith(
      authorizationUrl,
      "_blank",
      "noopener,noreferrer",
    );

    const callbackUrl =
      "http://localhost:1455/auth/callback?code=code-value&state=state-value";
    await user.type(screen.getByLabelText("Callback URL"), callbackUrl);
    await user.click(screen.getByRole("button", { name: "Complete connection" }));

    await waitFor(() =>
      expect(completeInput).toEqual({ callback_url: callbackUrl }),
    );
  });

  it("confirms and downloads a sensitive native credential export", async () => {
    seedAuthenticatedSession();
    let exportInput: CodexCredentialExportInput | undefined;
    const createObjectUrl = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:codex-export");
    const revokeObjectUrl = vi
      .spyOn(URL, "revokeObjectURL")
      .mockImplementation(() => undefined);
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => undefined);
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.post(
        "/console/v1/providers/codex-oauth/channel-groups/:id/credentials/export",
        async ({ request }) => {
          exportInput =
            (await request.json()) as CodexCredentialExportInput;
          return HttpResponse.json({
            type: "ai-gateway-codex-credentials",
            version: 1,
            exported_at: "2026-07-30T12:00:00.000Z",
            channel_group_id: GROUP_ID,
            channel_group_name: CODEX_GROUP.name,
            proxies: [],
            credentials: [
              {
                label: CREDENTIAL.label,
                email: CREDENTIAL.email,
                account_id: CREDENTIAL.account_id,
                user_id: CREDENTIAL.user_id,
                plan_type: CREDENTIAL.plan_type,
                is_fedramp: false,
                id_token: "secret-id",
                access_token: "secret-access",
                refresh_token: "secret-refresh",
                proxy_key: null,
                weight: 100,
                quota_threshold_percent: 95,
                enabled: true,
              },
            ],
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: "Export credentials" }),
    );
    await user.click(screen.getByRole("button", { name: "Export all" }));

    await waitFor(() =>
      expect(exportInput).toEqual({
        credential_ids: [],
        include_proxies: true,
      }),
    );
    expect(createObjectUrl).toHaveBeenCalled();
    expect(click).toHaveBeenCalled();
    expect(revokeObjectUrl).toHaveBeenCalledWith("blob:codex-export");
  });

  it("batch-disables selected credentials with their list versions", async () => {
    seedAuthenticatedSession();
    let batchInput: CodexCredentialBatchInput | undefined;
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.post(
        "/console/v1/providers/codex-oauth/channel-groups/:id/credentials/batch",
        async ({ request }) => {
          batchInput = (await request.json()) as CodexCredentialBatchInput;
          return HttpResponse.json({
            updated_ids: [CREDENTIAL_ID],
            correlation_id: "00000000-0000-0000-0000-00000000c005",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("checkbox", { name: "Select Personal Plus" }),
    );
    await user.click(screen.getByRole("button", { name: "Disable" }));

    await waitFor(() =>
      expect(batchInput).toEqual({
        items: [
          {
            id: CREDENTIAL_ID,
            updated_at: CREDENTIAL.updated_at,
          },
        ],
        operation: "disable",
      }),
    );
  });

  it("confirms credential deletion and sends If-Match", async () => {
    seedAuthenticatedSession();
    let ifMatch = "";
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.delete(
        "/console/v1/providers/codex-oauth/credentials/:id",
        ({ request }) => {
          ifMatch = request.headers.get("if-match") ?? "";
          return HttpResponse.json({
            id: CREDENTIAL_ID,
            correlation_id: "00000000-0000-0000-0000-00000000c006",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: "Delete Personal Plus" }),
    );
    await user.click(
      screen.getByRole("button", { name: "Delete credential" }),
    );

    await waitFor(() =>
      expect(ifMatch).toBe(`"${CREDENTIAL.updated_at}"`),
    );
  });
});
