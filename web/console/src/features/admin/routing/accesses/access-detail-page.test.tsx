import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { UPSTREAM_ACCESS } from "@/test/fixtures";
import type { UpstreamAccessInput } from "@/api/types";

function renderAt(id: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", `/admin/routing/accesses/${id}`);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

describe("upstream accesses", () => {
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
      name: "Independent access", connector_kind: "openai_compatible",
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
    expect(submitted?.connector_kind).toBe("openai_compatible");
    expect(submitted?.proxy_id).toBeNull();
    expect(ifMatch).toBe(`"${UPSTREAM_ACCESS.updated_at}"`);
  });

  it("reloads conflicts and blocks non-positive timeout input", async () => {
    let reads = 0;
    let writes = 0;
    server.use(
      http.get("/console/v1/routing/accesses/:id", () => {
        reads += 1;
        return HttpResponse.json(UPSTREAM_ACCESS, { headers: { ETag: `"${UPSTREAM_ACCESS.updated_at}"` } });
      }),
      http.put("/console/v1/routing/accesses/:id", () => {
        writes += 1;
        return HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAt(UPSTREAM_ACCESS.id);
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(UPSTREAM_ACCESS.name));
    await user.type(screen.getByLabelText("Connect timeout (ms)"), "0");
    await user.click(screen.getByRole("button", { name: "Save access" }));
    expect(writes).toBe(0);
    await user.clear(screen.getByLabelText("Connect timeout (ms)"));
    await user.click(screen.getByRole("button", { name: "Save access" }));
    await waitFor(() => expect(reads).toBeGreaterThan(1));
    expect(await screen.findByText("This access was changed elsewhere. Reloading.")).toBeInTheDocument();
  });
});
