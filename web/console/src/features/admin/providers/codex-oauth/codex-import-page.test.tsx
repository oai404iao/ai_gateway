import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import type {
  ChannelGroupView,
  CodexCredentialImportInput,
  ProxyCreateInput,
  ProxyInput,
} from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { CHANNEL_GROUP, PROXY } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

const GROUP_ID = "00000000-0000-0000-0000-00000000d001";
const NEW_PROXY_ID = "00000000-0000-0000-0000-00000000d002";
const CODEX_GROUP: ChannelGroupView = {
  ...CHANNEL_GROUP,
  id: GROUP_ID,
  name: "Portable Codex",
  api_format: "open_ai_responses",
  connector_kind: "codex_oauth",
  connector_pool_id: GROUP_ID,
};

function renderPage() {
  window.history.replaceState(
    {},
    "",
    `/admin/providers/codex-oauth/${GROUP_ID}/import`,
  );
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("CodexImportPage", () => {
  it("keeps CPA JSON in an editable draft and imports it with an existing proxy", async () => {
    seedAuthenticatedSession();
    let submitted: CodexCredentialImportInput | undefined;
    server.use(
      http.get("/console/v1/routing/channel-groups/:id", () =>
        HttpResponse.json(CODEX_GROUP, {
          headers: { ETag: `"${CODEX_GROUP.updated_at}"` },
        }),
      ),
      http.post(
        "/console/v1/providers/codex-oauth/channel-groups/:id/credentials",
        async ({ request }) => {
          submitted = (await request.json()) as CodexCredentialImportInput;
          return HttpResponse.json(
            {
              id: "00000000-0000-0000-0000-00000000d003",
              correlation_id: "00000000-0000-0000-0000-00000000d004",
            },
            { status: 201 },
          );
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("heading", { name: "Advanced Codex import" });
    fireEvent.change(screen.getByLabelText("Credential JSON"), {
      target: {
        value: JSON.stringify({
        type: "codex",
        email: "cpa@example.test",
        account_id: "account-cpa",
        user_id: "user-cpa",
        id_token: "id-token",
        access_token: "access-token",
        refresh_token: "refresh-token",
        proxy_url: PROXY.proxy_url,
        }),
      },
    });
    await user.click(screen.getByRole("button", { name: "Parse into drafts" }));

    expect(
      await screen.findByDisplayValue("cpa@example.test"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Proxy for cpa@example.test" }),
    ).toHaveTextContent(PROXY.name);

    await user.click(
      screen.getByRole("button", { name: "Validate and import selected" }),
    );
    await waitFor(() =>
      expect(submitted).toMatchObject({
        label: "cpa@example.test",
        enabled: true,
        proxy_id: PROXY.id,
        weight: 100,
        quota_threshold_percent: 95,
        account_id: "account-cpa",
        user_id: "user-cpa",
        id_token: "id-token",
        access_token: "access-token",
        refresh_token: "refresh-token",
      }),
    );
    expect(await screen.findByText("Imported")).toBeInTheDocument();
  });

  it("uploads a Sub2API bundle, reviews its proxy, creates it, and assigns it before import", async () => {
    seedAuthenticatedSession();
    let proxyInput: ProxyCreateInput | undefined;
    let credentialInput: CodexCredentialImportInput | undefined;
    server.use(
      http.get("/console/v1/routing/channel-groups/:id", () =>
        HttpResponse.json(CODEX_GROUP, {
          headers: { ETag: `"${CODEX_GROUP.updated_at}"` },
        }),
      ),
      http.get("/console/v1/network/proxies", () => HttpResponse.json([])),
      http.post("/console/v1/network/proxies", async ({ request }) => {
        proxyInput = (await request.json()) as ProxyCreateInput;
        return HttpResponse.json(
          {
            id: NEW_PROXY_ID,
            correlation_id: "00000000-0000-0000-0000-00000000d005",
          },
          { status: 201 },
        );
      }),
      http.post(
        "/console/v1/providers/codex-oauth/channel-groups/:id/credentials",
        async ({ request }) => {
          credentialInput =
            (await request.json()) as CodexCredentialImportInput;
          return HttpResponse.json(
            {
              id: "00000000-0000-0000-0000-00000000d006",
              correlation_id: "00000000-0000-0000-0000-00000000d007",
            },
            { status: 201 },
          );
        },
      ),
    );
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("heading", { name: "Advanced Codex import" });
    await user.click(screen.getByRole("tab", { name: "JSON files" }));
    const file = new File(
      [
        JSON.stringify({
          code: 0,
          data: {
            type: "sub2api-data",
            version: 1,
            proxies: [
              {
                proxy_key: "socks5|10.0.0.8|1080|user|pass",
                name: "Imported egress",
                protocol: "socks5",
                host: "10.0.0.8",
                port: 1080,
                username: "user",
                password: "pass",
                status: "active",
              },
            ],
            accounts: [
              {
                name: "Sub2API Plus",
                platform: "openai",
                type: "oauth",
                proxy_key: "socks5|10.0.0.8|1080|user|pass",
                credentials: {
                  access_token: "sub2-access",
                  refresh_token: "sub2-refresh",
                  chatgpt_account_id: "sub2-account",
                  chatgpt_user_id: "sub2-user",
                },
              },
            ],
          },
        }),
      ],
      "sub2api-export.json",
      { type: "application/json" },
    );
    await user.upload(
      screen.getByLabelText("JSON files", { selector: "input" }),
      file,
    );
    await user.click(screen.getByRole("button", { name: "Parse into drafts" }));

    expect(await screen.findByText("Imported egress")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Review and create" }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByDisplayValue("socks5://10.0.0.8:1080"),
    ).toBeInTheDocument();
    await user.click(
      within(dialog).getByRole("button", { name: "Create proxy" }),
    );
    await waitFor(() =>
      expect(proxyInput).toMatchObject({
        name: "Imported egress",
        proxy_url: "socks5://10.0.0.8:1080",
        username: "user",
        password: "pass",
        enabled: true,
      }),
    );

    await user.click(
      screen.getByRole("button", { name: "Validate and import selected" }),
    );
    await waitFor(() =>
      expect(credentialInput).toMatchObject({
        label: "Sub2API Plus",
        proxy_id: NEW_PROXY_ID,
        account_id: "sub2-account",
        user_id: "sub2-user",
        access_token: "sub2-access",
        refresh_token: "sub2-refresh",
      }),
    );
  });

  it("edits and deletes an existing proxy without leaving the import page", async () => {
    seedAuthenticatedSession();
    let updateInput: ProxyInput | undefined;
    let updateIfMatch = "";
    let deleteIfMatch = "";
    server.use(
      http.get("/console/v1/routing/channel-groups/:id", () =>
        HttpResponse.json(CODEX_GROUP, {
          headers: { ETag: `"${CODEX_GROUP.updated_at}"` },
        }),
      ),
      http.get("/console/v1/network/proxies", () =>
        HttpResponse.json([PROXY]),
      ),
      http.put(
        "/console/v1/network/proxies/:id",
        async ({ request }) => {
          updateIfMatch = request.headers.get("if-match") ?? "";
          updateInput = (await request.json()) as ProxyInput;
          return new HttpResponse(null, { status: 204 });
        },
      ),
      http.delete("/console/v1/network/proxies/:id", ({ request }) => {
        deleteIfMatch = request.headers.get("if-match") ?? "";
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole("heading", { name: "Advanced Codex import" });
    const proxyHeading = await screen.findByText("Proxy configuration");
    expect(proxyHeading.closest('[data-slot="card"]')).toHaveTextContent(
      PROXY.name,
    );
    await user.click(
      screen.getByRole("button", { name: `Edit ${PROXY.name}` }),
    );
    const editDialog = await screen.findByRole("dialog");
    const nameInput = within(editDialog).getByLabelText("Name");
    await user.clear(nameInput);
    await user.type(nameInput, "Edited egress");
    await user.click(
      within(editDialog).getByRole("button", { name: "Save proxy" }),
    );

    await waitFor(() =>
      expect(updateInput).toMatchObject({
        name: "Edited egress",
        proxy_url: PROXY.proxy_url,
        enabled: true,
      }),
    );
    expect(updateIfMatch).toBe(`"${PROXY.updated_at}"`);

    await user.click(
      screen.getByRole("button", { name: `Delete ${PROXY.name}` }),
    );
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Delete proxy",
      }),
    );
    await waitFor(() => expect(deleteIfMatch).toBe(`"${PROXY.updated_at}"`));
  });
});
