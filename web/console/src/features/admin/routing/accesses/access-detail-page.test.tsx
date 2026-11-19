import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CONNECTOR_PLUGINS, UPSTREAM_ACCESS } from "@/test/fixtures";
import type { UpstreamAccessInput } from "@/api/types";

function renderAt(id: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", `/admin/routing/accesses/${id}`);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

describe("upstream accesses", () => {
  it("selects a registered external provider without changing authentication or topology", async () => {
    let submitted: UpstreamAccessInput | undefined;
    server.use(
      http.get("/console/v1/system/connectors", () => HttpResponse.json([
        ...CONNECTOR_PLUGINS,
        { id: "acme", version: "2.0.0", abi_version: 1, built_in: false, operations: ["responses"] },
      ])),
      http.post("/console/v1/routing/accesses", async ({ request }) => {
        submitted = await request.json() as UpstreamAccessInput;
        return HttpResponse.json({ id: UPSTREAM_ACCESS.id }, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAt("new");
    await user.type(await screen.findByLabelText("Name"), "External access");
    await user.type(screen.getByLabelText("Base URL"), "https://api.example.test");
    await user.click(screen.getByRole("combobox", { name: "Connector" }));
    await user.click(await screen.findByRole("option", { name: "acme" }));
    expect(screen.getByText("acme · 2.0.0 · Native plugin")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(submitted?.connector_kind).toBe("acme"));
    expect(submitted?.enabled).toBe(false);
  });
  it("creates a disabled access without reselecting seeded selects or adding credentials", async () => {
    let submitted: UpstreamAccessInput | undefined;
    server.use(http.post("/console/v1/routing/accesses", async ({ request }) => {
      submitted = await request.json() as UpstreamAccessInput;
      return HttpResponse.json({ id: UPSTREAM_ACCESS.id }, { status: 201 });
    }));
    const user = userEvent.setup();
    renderAt("new");
    await user.type(await screen.findByLabelText("Name"), "Independent access");
    await user.type(screen.getByLabelText("Base URL"), "https://api.example.test");
    await user.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(submitted).toEqual({
      name: "Independent access", connector_kind: "general",
      base_url: "https://api.example.test", proxy_id: null,
      connect_timeout_ms: null, response_header_timeout_ms: null,
      stream_idle_timeout_ms: null, enabled: false,
    }));
  });

  it("preserves loaded network settings and sends If-Match without touching select fields", async () => {
    let submitted: UpstreamAccessInput | undefined;
    let ifMatch: string | null = null;
    server.use(http.put("/console/v1/routing/accesses/:id", async ({ request }) => {
      submitted = await request.json() as UpstreamAccessInput;
      ifMatch = request.headers.get("if-match");
      return HttpResponse.json({ id: UPSTREAM_ACCESS.id });
    }));
    const user = userEvent.setup();
    renderAt(UPSTREAM_ACCESS.id);
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(UPSTREAM_ACCESS.name));
    expect(screen.getByLabelText("Connector")).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(submitted?.response_header_timeout_ms).toBe(30000));
    expect(submitted?.connector_kind).toBe("general");
    expect(submitted?.proxy_id).toBeNull();
    expect(ifMatch).toBe(`"${UPSTREAM_ACCESS.updated_at}"`);
  });

  it("reloads conflicts and blocks non-positive timeout input", async () => {
    const writes: { ifMatch: string | null; name: string }[] = [];
    const editedElsewhere = { ...UPSTREAM_ACCESS, name: "New upstream access", updated_at: "2026-08-02T00:00:00Z" };
    const path = `/console/v1/routing/accesses/${UPSTREAM_ACCESS.id}`;
    server.use(
      http.get(path, () => {
        const current = writes.length ? editedElsewhere : UPSTREAM_ACCESS;
        return HttpResponse.json(current, { headers: { ETag: `"${current.updated_at}"` } });
      }),
      http.put(path, async ({ request }) => {
        const { name } = await request.json() as UpstreamAccessInput;
        const ifMatch = request.headers.get("if-match");
        writes.push({ ifMatch, name });
        return ifMatch === `"${editedElsewhere.updated_at}"`
          ? HttpResponse.json({ id: UPSTREAM_ACCESS.id })
          : HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAt(UPSTREAM_ACCESS.id);
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(UPSTREAM_ACCESS.name));
    await user.type(screen.getByLabelText("Connect timeout (ms)"), "0");
    await user.click(screen.getByRole("button", { name: "Save access" }));
    expect(writes).toEqual([]);
    await user.clear(screen.getByLabelText("Connect timeout (ms)"));
    await user.click(screen.getByRole("button", { name: "Save access" }));
    expect(await screen.findByText("This access was changed elsewhere. Reloading.")).toBeInTheDocument();
    expect(await screen.findByDisplayValue(editedElsewhere.name)).toBeVisible();
    expect(writes).toEqual([{ ifMatch: `"${UPSTREAM_ACCESS.updated_at}"`, name: UPSTREAM_ACCESS.name }]);
    await user.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[1]).toEqual({ ifMatch: `"${editedElsewhere.updated_at}"`, name: editedElsewhere.name });
  });
});
