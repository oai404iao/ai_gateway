import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type {
  RoutingGroupView,
  CodexCredentialExportBundle,
  CodexCredentialExportInput,
  CodexCredentialUpdateInput,
  CodexCredentialView,
  CodexOauthCompleteInput,
  CodexOauthStartInput,
} from "@/api/types";
import { PROXY, ROUTING_GROUP } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

const GROUP_ID = "00000000-0000-0000-0000-00000000c001";
const CREDENTIAL_ID = "00000000-0000-0000-0000-00000000c002";
const FLOW_ID = "00000000-0000-0000-0000-00000000c003";

const CODEX_GROUP: RoutingGroupView = {
  ...ROUTING_GROUP,
  id: GROUP_ID,
  name: "Codex subscriptions",
};

const CREDENTIAL: CodexCredentialView = {
  id: CREDENTIAL_ID,
  channel_ids: [],
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
  primary_window_cost_amount: "4.75",
  secondary_used_percent: null,
  secondary_window_seconds: null,
  secondary_reset_at: null,
  secondary_window_cost_amount: null,
  quota_reset_credits_available: 2,
  quota_checked_at: "2026-07-29T12:00:00.000Z",
  last_error_code: null,
  last_error_summary: null,
  proxy_id: null,
  enabled: true,
  available_models: ["gpt-5-codex"],
  created_at: "2026-07-29T12:00:00.000Z",
  updated_at: "2026-07-29T12:00:00.000Z",
};

function renderPage() {
  window.history.replaceState({}, "", "/admin/routing/upstream-credentials?connector=codex");
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
    http.get("/console/v1/routing/groups/:id", () =>
      HttpResponse.json(CODEX_GROUP, {
        headers: { ETag: `"${CODEX_GROUP.updated_at}"` },
      }),
    ),
    http.get(
      "/console/v1/routing/upstream-credentials/codex",
      () => HttpResponse.json(credentials),
    ),
  ];
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("CodexOauthPage", () => {
  it("manages detached credentials without reading a routing group", async () => {
    seedAuthenticatedSession();
    const groupReads: string[] = [];
    const recordGroupRead = ({ request }: { request: Request }) => {
      groupReads.push(new URL(request.url).pathname);
      return HttpResponse.json({ error: "not_found" }, { status: 404 });
    };
    server.use(...baseHandlers([]));
    server.use(
      http.get("/console/v1/routing/groups", recordGroupRead),
      http.get("/console/v1/routing/groups/:id", recordGroupRead),
    );
    renderPage();

    expect(await screen.findByText("No Codex credentials")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect account" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Back to groups" })).not.toBeInTheDocument();
    expect(groupReads).toEqual([]);
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
    expect(screen.getByText("Personal Codex account")).toBeVisible();
    expect(screen.queryByText("Workspace account-123")).not.toBeInTheDocument();
  });

  it("names the maintenance proxy and keeps workspace and member IDs in account details", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    server.use(...baseHandlers([{ ...CREDENTIAL, proxy_id: PROXY.id }]));
    renderPage();
    const row = within(await screen.findByRole("row", { name: /Personal Plus/ }));
    expect(await row.findByText(PROXY.name)).toBeVisible();
    expect(row.getByText("Workspace account-123")).not.toBeVisible();
    await user.click(row.getByText("Workspace account"));
    expect(row.getByText("Workspace account-123")).toBeVisible();
    expect(row.getByText("Member user-123")).toBeVisible();
  });

  it("shows period spend for each quota window without a system-wide cost link", async () => {
    seedAuthenticatedSession();
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.get(
        "/console/v1/routing/upstream-credentials/codex/:id/quota/windows",
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
                cost_amount: "4.75",
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
                cost_amount: "24.50",
              },
            ],
          }),
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", {
        name: "View quota history for Personal Plus",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("heading", {
        name: "Quota window history for Personal Plus",
      }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("Manual reset credit")).toBeInTheDocument();
    expect(within(dialog).getByText("4.947917 USD")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "View costs" })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("tab", {name: /Secondary window/}));
    expect(within(dialog).getByText("OpenAI official reset")).toBeVisible();
    expect(within(dialog).getByText("24.50 USD")).toBeVisible();
  });

  it("shows credential action tooltips on focus and dismisses them on blur", async () => {
    seedAuthenticatedSession();
    server.use(...baseHandlers([CREDENTIAL]));
    renderPage();

    await screen.findByText("Personal Plus");

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

  it("edits credential settings without displaying or submitting weight", async () => {
    seedAuthenticatedSession();
    let updateInput: CodexCredentialUpdateInput | undefined;
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.get(
        "/console/v1/routing/upstream-credentials/codex/:id",
        () =>
          HttpResponse.json(CREDENTIAL, {
            headers: { ETag: `"${CREDENTIAL.updated_at}"` },
          }),
      ),
      http.put(
        "/console/v1/routing/upstream-credentials/codex/:id",
        async ({ request }) => {
          updateInput = (await request.json()) as CodexCredentialUpdateInput;
          return HttpResponse.json({
            id: CREDENTIAL_ID,
            correlation_id: "00000000-0000-0000-0000-00000000c013",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: "Edit Personal Plus" }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText("Weight")).not.toBeInTheDocument();
    await user.click(
      within(dialog).getByRole("button", { name: "Save changes" }),
    );

    await waitFor(() =>
      expect(updateInput).toEqual({
        label: "Personal Plus",
        enabled: true,
        proxy_id: null,
        quota_threshold_percent: 95,
      }),
    );
    expect(updateInput).not.toHaveProperty("weight");
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
        "/console/v1/routing/upstream-credentials/codex/oauth/flows",
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
        "/console/v1/routing/upstream-credentials/codex/oauth/flows/:id/complete",
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
    expect(screen.queryByLabelText("Weight")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Start authorization" }));

    await waitFor(() => expect(startInput?.label).toBe("Personal Plus"));
    expect(startInput).toMatchObject({
      proxy_id: null,
      quota_threshold_percent: 95,
    });
    expect(startInput).not.toHaveProperty("weight");
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

  it.each(["selected", "all"] as const)("confirms and downloads %s native credentials as JSON", async (mode) => {
    seedAuthenticatedSession();
    let exportInput: CodexCredentialExportInput | undefined;
    const ids = mode === "selected" ? [CREDENTIAL_ID] : [];
    const createObjectUrl = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:codex-export");
    const revokeObjectUrl = vi
      .spyOn(URL, "revokeObjectURL")
      .mockImplementation(() => undefined);
    const downloads: { href: string; filename: string }[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      downloads.push({ href: this.href, filename: this.download });
    });
    server.use(
      ...baseHandlers(mode === "all" ? [CREDENTIAL] : [
        CREDENTIAL, { ...CREDENTIAL, id: "00000000-0000-0000-0000-00000000c004", label: "Unselected credential" },
      ]),
      http.post(
        "/console/v1/routing/upstream-credentials/codex/export",
        async ({ request }) => {
          exportInput =
            (await request.json()) as CodexCredentialExportInput;
          if (exportInput.credential_ids.length !== ids.length || ids.some((id, index) => id !== exportInput?.credential_ids[index])) {
            return HttpResponse.json({ error: "unexpected_credential_selection" }, { status: 400 });
          }
          return HttpResponse.json({
            type: "ai-gateway-codex-credentials",
            version: 3,
            exported_at: "2026-07-30T12:00:00.000Z",
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
                quota_threshold_percent: 95,
                enabled: true,
              },
            ],
          } satisfies CodexCredentialExportBundle);
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    if (mode === "selected") {
      await user.click(await screen.findByRole("checkbox", { name: "Select Personal Plus" }));
      await user.click(screen.getByRole("button", { name: "Export selected (1)" }));
    } else {
      await user.click(await screen.findByRole("button", { name: "Export credentials" }));
    }
    const confirmation = screen.getByRole("alertdialog");
    expect(within(confirmation).getByText(/contains raw ID, access, refresh, and proxy credentials/)).toBeVisible();
    expect(exportInput).toBeUndefined();
    await user.click(within(confirmation).getByRole("button", { name: `Export ${mode}` }));

    expect(await screen.findByText("Codex credentials exported.")).toBeVisible();
    expect(exportInput).toEqual({ credential_ids: ids, include_proxies: true });
    expect(createObjectUrl).toHaveBeenCalledTimes(1);
    const [blob] = createObjectUrl.mock.calls[0];
    if (!(blob instanceof Blob)) throw new Error("Expected an export Blob");
    expect(blob.type).toBe("application/json");
    const { type, version, credentials } = JSON.parse(await blob.text()) as CodexCredentialExportBundle;
    expect({ type, version, identities: credentials.map(({ label, email, account_id }) => ({ label, email, account_id })) }).toEqual({
      type: "ai-gateway-codex-credentials",
      version: 3,
      identities: [{ label: "Personal Plus", email: "codex@example.test", account_id: "account-123" }],
    });
    const { id_token, access_token, refresh_token } = credentials[0];
    expect(id_token === "secret-id" && access_token === "secret-access" && refresh_token === "secret-refresh").toBe(true);
    expect(downloads).toEqual([{
      href: "blob:codex-export",
      filename: expect.stringMatching(/^codex-credentials-\d{4}-\d{2}-\d{2}\.json$/),
    }]);
    expect(revokeObjectUrl).toHaveBeenCalledWith("blob:codex-export");
  });

  it("confirms credential deletion and sends If-Match", async () => {
    seedAuthenticatedSession();
    let ifMatch = "";
    server.use(
      ...baseHandlers([CREDENTIAL]),
      http.delete(
        "/console/v1/routing/upstream-credentials/codex/:id",
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
