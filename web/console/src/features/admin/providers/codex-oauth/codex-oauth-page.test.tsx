import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type {
  ChannelGroupView,
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
};

const CREDENTIAL: CodexCredentialView = {
  id: CREDENTIAL_ID,
  channel_group_id: GROUP_ID,
  label: "Personal Plus",
  email: "codex@example.test",
  account_id: "account-123",
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

  it("exposes tooltip descriptions for credential actions", async () => {
    seedAuthenticatedSession();
    server.use(...baseHandlers([CREDENTIAL]));
    renderPage();

    expect(await screen.findByText("Personal Plus")).toBeInTheDocument();

    for (const label of [
      "Edit Personal Plus",
      "Refresh quota for Personal Plus",
      "Refresh token for Personal Plus",
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

  it("starts PKCE authorization and submits the copied callback URL", async () => {
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
});
