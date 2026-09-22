import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL, UPSTREAM_ACCESS } from "@/test/fixtures";
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

  it("lists operation capabilities without configurable transports", async () => {
    renderAt("/admin/routing/capabilities");
    expect(await screen.findByText("Primary channel")).toBeInTheDocument();
    expect(screen.getAllByText("Responses").length).toBeGreaterThan(0);
    expect(screen.queryByText("Transports")).not.toBeInTheDocument();
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
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
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
    let reads = 0;
    let writes = 0;
    server.use(
      http.get("/console/v1/routing/capabilities/:id", () => {
        reads += 1;
        return HttpResponse.json({
          ...CHANNEL_CAPABILITY, auto_disabled: true, auto_disable_reason: "HTTP 429",
        }, { headers: { ETag: `"${CHANNEL_CAPABILITY.updated_at}"` } });
      }),
      http.post("/console/v1/routing/capabilities/:id/recover", () => {
        writes += 1;
        return HttpResponse.json({ error: { code: "conflict", message: "Changed" } }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
    await user.click(await screen.findByRole("button", { name: "Recover capability" }));
    await user.click(screen.getByRole("button", { name: "Recover" }));
    await waitFor(() => expect(reads).toBe(2));
    expect(writes).toBe(1);
    expect(screen.getByText("HTTP 429")).toBeInTheDocument();
  });
});
