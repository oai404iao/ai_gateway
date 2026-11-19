import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL, UPSTREAM_ACCESS, UPSTREAM_CREDENTIAL } from "@/test/fixtures";
import type { CapabilityBatchUpdateInput, ChannelCapabilityInput, ChannelModelDiscoveryInput } from "@/api/types";

function renderAt(path: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("channel capabilities", () => {
  it("limits operation choices to the selected provider manifest", async () => {
    server.use(
      http.get("/console/v1/system/connectors", () => HttpResponse.json([
        { id: "acme", version: "1.0.0", abi_version: 1, built_in: false, operations: ["responses"] },
      ])),
      http.get("/console/v1/routing/accesses", () =>
        HttpResponse.json([{ ...UPSTREAM_ACCESS, connector_kind: "acme" }])),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}?view=capabilities&capability=new`);
    await waitFor(() => expect(screen.getByRole("button", { name: "Save capability" })).toBeEnabled());
    await user.click(screen.getByRole("combobox", { name: "Operation" }));
    expect(await screen.findByRole("option", { name: "Responses" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Responses WebSocket" })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Images generation" })).not.toBeInTheDocument();
  });
  it("creates a capability inside its channel without changing the binding", async () => {
    let submitted: ChannelCapabilityInput | undefined;
    server.use(http.post("/console/v1/routing/capabilities", async ({ request }) => {
      submitted = await request.json() as ChannelCapabilityInput;
      return HttpResponse.json({ id: CHANNEL_CAPABILITY.id }, { status: 201 });
    }));
    const user = userEvent.setup();
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}?view=capabilities&capability=new`);
    await user.click(await screen.findByRole("combobox", { name: "Operation" }));
    await user.click(await screen.findByRole("option", { name: "Responses WebSocket" }));
    await user.click(screen.getByRole("button", { name: "Save capability" }));
    await waitFor(() => expect(submitted?.channel_id).toBe(LOGICAL_CHANNEL.id));
    expect(submitted?.settings.operation).toBe("responses-ws");
    expect(submitted?.settings).not.toHaveProperty("transports");
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("capability")).toBe(CHANNEL_CAPABILITY.id));
    expect(window.location.pathname).toBe(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`);
  });

  it("discovers through the selected access and credential without saving until confirmed", async () => {
    let discovery: ChannelModelDiscoveryInput | undefined;
    let saved: ChannelCapabilityInput | undefined;
    server.use(
      http.post("/console/v1/routing/channels/models/discover", async ({ request }) => {
        discovery = await request.json() as ChannelModelDiscoveryInput;
        return HttpResponse.json({ models: ["gpt-5", "new-wire-model"] });
      }),
      http.put("/console/v1/routing/capabilities/:id", async ({ request }) => {
        saved = await request.json() as ChannelCapabilityInput;
        return HttpResponse.json({ id: CHANNEL_CAPABILITY.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
    await user.click(await screen.findByRole("button", { name: "Fetch models" }));
    await user.click(await screen.findByRole("checkbox", { name: "Select new-wire-model" }));
    await user.click(screen.getByRole("button", { name: "Apply selection" }));
    expect(saved).toBeUndefined();
    expect(discovery).toMatchObject({
      api_format: "open_ai_responses", base_url: UPSTREAM_ACCESS.base_url,
      credential_id: LOGICAL_CHANNEL.credential_id,
      response_header_timeout_ms: UPSTREAM_ACCESS.response_header_timeout_ms,
      override_document: {},
    });
    await user.click(screen.getByRole("button", { name: "Save capability" }));
    await waitFor(() => expect(saved?.settings.available_models).toEqual([
      "gpt-5", "gpt-5-mini", "new-wire-model",
    ]));
  });

  it("fetches models for a codex-credentialed channel through the shared discovery endpoint", async () => {
    let discovery: ChannelModelDiscoveryInput | undefined;
    const oauth = {
      ...UPSTREAM_CREDENTIAL,
      id: "00000000-0000-0000-0000-000000001801",
      connector_kind: "codex",
      kind: "codex_oauth",
      provider_managed: true,
      channel_ids: [LOGICAL_CHANNEL.id],
      allowed_base_urls: ["https://codex.test"],
    };
    server.use(
      http.get("/console/v1/routing/accesses", () =>
        HttpResponse.json([{ ...UPSTREAM_ACCESS, connector_kind: "codex", base_url: "https://codex.test" }]),
      ),
      http.get("/console/v1/routing/logical-channels", () =>
        HttpResponse.json([{ ...LOGICAL_CHANNEL, credential_id: oauth.id }])),
      http.get("/console/v1/routing/upstream-credentials", () => HttpResponse.json([UPSTREAM_CREDENTIAL, oauth])),
      http.post("/console/v1/routing/channels/models/discover", async ({ request }) => {
        discovery = await request.json() as ChannelModelDiscoveryInput;
        return HttpResponse.json({ models: ["gpt-5-codex"] });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
    await user.click(await screen.findByRole("button", { name: "Fetch models" }));
    await waitFor(() => expect(discovery).toMatchObject({
      credential_id: oauth.id,
      base_url: "https://codex.test",
    }));
    expect(await screen.findByRole("checkbox", { name: "Select gpt-5-codex" })).toBeInTheDocument();
  });

  it("preserves operation settings and sends If-Match on update", async () => {
    let submitted: ChannelCapabilityInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/routing/capabilities/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelCapabilityInput;
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({ id: CHANNEL_CAPABILITY.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}?view=capabilities&capability=${CHANNEL_CAPABILITY.id}`);
    await waitFor(() =>
      expect(screen.getByLabelText("Logical channel")).toHaveTextContent(
        LOGICAL_CHANNEL.name,
      ),
    );
    await user.click(screen.getByRole("button", { name: "Save capability" }));
    await waitFor(() =>
      expect(submitted?.settings.available_models).toEqual(["gpt-5", "gpt-5-mini"]),
    );
    expect(submitted?.channel_id).toBe(LOGICAL_CHANNEL.id);
    expect(submitted?.settings.operation).toBe("responses");
    expect(submitted?.settings).not.toHaveProperty("transports");
    expect(ifMatch).toBe(`"${CHANNEL_CAPABILITY.updated_at}"`);
    expect(window.location.pathname).toBe(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`);
  });

  it("does not embed another channel's capability through a mismatched URL", async () => {
    server.use(http.get("/console/v1/routing/capabilities/:id", () => HttpResponse.json({
      ...CHANNEL_CAPABILITY, channel_id: "00000000-0000-4000-8000-000000000099",
    }, { headers: { ETag: `"${CHANNEL_CAPABILITY.updated_at}"` } })));
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}?view=capabilities&capability=${CHANNEL_CAPABILITY.id}`);
    expect(await screen.findByText("This capability belongs to another channel.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save capability" })).not.toBeInTheDocument();
  });

  it("confirms route withdrawal before deleting a capability with its version", async () => {
    const deletes: { id: string; ifMatch: string | null }[] = [];
    server.use(http.delete("/console/v1/routing/capabilities/:id", ({ request, params }) => {
      deletes.push({ id: String(params.id), ifMatch: request.headers.get("if-match") });
      return new HttpResponse(null, { status: 204 });
    }));
    const user = userEvent.setup();
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}?view=capabilities&capability=${CHANNEL_CAPABILITY.id}`);
    await user.click(await screen.findByRole("button", { name: "Delete capability" }));
    expect(screen.getByText("This deletes the capability and all routing candidates referencing it. Empty route rules are disabled. Channel authorization and billing history are preserved.")).toBeInTheDocument();
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(deletes).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Delete capability" }));
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).has("capability")).toBe(false));
    expect(deletes).toEqual([{ id: CHANNEL_CAPABILITY.id, ifMatch: `"${CHANNEL_CAPABILITY.updated_at}"` }]);
    expect(window.location.pathname).toBe(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`);
    expect(new URLSearchParams(window.location.search).get("view")).toBe("capabilities");
  });

  it("selects capabilities without navigating and batches only changed fields with their versions", async () => {
    let submitted: CapabilityBatchUpdateInput | undefined;
    server.use(http.post("/console/v1/routing/capabilities/batch", async ({ request }) => {
      submitted = await request.json() as CapabilityBatchUpdateInput;
      return HttpResponse.json({ updated_ids: [CHANNEL_CAPABILITY.id], correlation_id: "audit" });
    }));
    const user = userEvent.setup();
    renderAt("/admin/routing/capabilities");
    await user.click(await screen.findByRole("checkbox", { name: "Select Primary channel / Responses" }));
    expect(window.location.pathname).toBe("/admin/routing/capabilities");
    await user.click(screen.getByRole("button", { name: "Batch edit capabilities (1/100)" }));
    await user.click(screen.getByRole("button", { name: "Apply changes" }));
    expect(submitted).toBeUndefined();
    expect(await screen.findByText("Choose at least one field to change.")).toBeInTheDocument();
    await user.type(screen.getByLabelText("Billing multiplier"), "1.75");
    await user.click(screen.getByRole("button", { name: "Apply changes" }));
    await waitFor(() => expect(submitted).toEqual({
      items: [{ id: CHANNEL_CAPABILITY.id, updated_at: CHANNEL_CAPABILITY.updated_at }],
      changes: { billing_multiplier: "1.75" },
    }));
    expect(await screen.findByRole("button", { name: "Batch edit capabilities (0/100)" })).toBeDisabled();
  });

  it("recovers automatic-disable state using the capability ETag, without enabling it", async () => {
    let recovered = false;
    let ifMatch: string | null = null;
    server.use(
      http.get("/console/v1/routing/capabilities/:id", () => HttpResponse.json({
        ...CHANNEL_CAPABILITY,
        settings: { ...CHANNEL_CAPABILITY.settings, enabled: false },
        auto_disabled: !recovered,
        auto_disable_reason: recovered ? null : "HTTP 429",
      }, { headers: { ETag: `"${CHANNEL_CAPABILITY.updated_at}"` } })),
      http.post("/console/v1/routing/capabilities/:id/recover", ({ request }) => {
        ifMatch = request.headers.get("if-match");
        recovered = true;
        return HttpResponse.json({ id: CHANNEL_CAPABILITY.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
    await user.click(await screen.findByRole("button", { name: "Recover capability" }));
    await user.click(screen.getByRole("button", { name: "Recover" }));
    await waitFor(() => expect(ifMatch).toBe(`"${CHANNEL_CAPABILITY.updated_at}"`));
    await waitFor(() => expect(screen.queryByText("HTTP 429")).not.toBeInTheDocument());
    expect(screen.getByRole("switch", { name: "Enabled" })).not.toBeChecked();
  });

  it("reloads a stale recovery instead of retrying the write", async () => {
    const versions: (string | null)[] = [];
    const updatedAt = "2026-08-02T00:00:00Z";
    const path = `/console/v1/routing/capabilities/${CHANNEL_CAPABILITY.id}`;
    server.use(
      http.get(path, () => {
        const updated_at = versions.length ? updatedAt : CHANNEL_CAPABILITY.updated_at;
        return HttpResponse.json({
          ...CHANNEL_CAPABILITY, updated_at, auto_disabled: versions.length < 2,
          auto_disable_reason: versions.length === 0 ? "HTTP 429" : versions.length === 1 ? "HTTP 503" : null,
        }, { headers: { ETag: `"${updated_at}"` } });
      }),
      http.post(`${path}/recover`, ({ request }) => {
        const ifMatch = request.headers.get("if-match");
        versions.push(ifMatch);
        return ifMatch === `"${updatedAt}"`
          ? HttpResponse.json({ id: CHANNEL_CAPABILITY.id })
          : HttpResponse.json({ error: { code: "conflict", message: "Changed" } }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
    expect(await screen.findByText("HTTP 429")).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Recover capability" }));
    await user.click(screen.getByRole("button", { name: "Recover" }));
    expect(await screen.findByText("HTTP 503")).toBeInTheDocument();
    expect(screen.queryByText("HTTP 429")).not.toBeInTheDocument();
    expect(versions).toEqual([`"${CHANNEL_CAPABILITY.updated_at}"`]);
    await user.click(screen.getByRole("button", { name: "Recover" }));
    expect(await screen.findByText("Capability recovered")).toBeVisible();
    expect(versions).toEqual([`"${CHANNEL_CAPABILITY.updated_at}"`, `"${updatedAt}"`]);
  });
});
