import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { UPSTREAM_CREDENTIAL, UPSTREAM_CREDENTIAL_DETAIL } from "@/test/fixtures";
import type { UpstreamCredentialInput } from "@/api/types";

function renderAt(id: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", `/admin/routing/upstream-credentials/${id}`);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

describe("upstream credential management", () => {
  it("creates a scoped credential without reselecting the default authentication type", async () => {
    let submitted: UpstreamCredentialInput | undefined;
    server.use(http.post("/console/v1/routing/upstream-credentials", async ({ request }) => {
      submitted = await request.json() as UpstreamCredentialInput;
      return HttpResponse.json({ id: UPSTREAM_CREDENTIAL.id, correlation_id: "test-create" }, { status: 201 });
    }));
    const user = userEvent.setup();
    renderAt("new");
    await user.type(await screen.findByLabelText("Name"), "Reusable identity");
    await user.type(screen.getByLabelText("Credential secret"), "test-new-secret");
    await user.type(screen.getByLabelText("Allowed Base URLs"), "https://api.example.test");
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(submitted).toEqual({
      name: "Reusable identity", kind: "bearer", header_name: null,
      secret: "test-new-secret", allowed_base_urls: ["https://api.example.test"], enabled: true,
    }));
  });

  it("preserves or rotates a shared secret explicitly and sends the detail ETag", async () => {
    const submitted: UpstreamCredentialInput[] = [];
    let ifMatch: string | null = null;
    server.use(http.put("/console/v1/routing/upstream-credentials/:id", async ({ request }) => {
      submitted.push(await request.json() as UpstreamCredentialInput);
      ifMatch = request.headers.get("if-match");
      return HttpResponse.json({ id: UPSTREAM_CREDENTIAL.id, correlation_id: "test-update" });
    }));
    const user = userEvent.setup();
    renderAt(UPSTREAM_CREDENTIAL.id);
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(UPSTREAM_CREDENTIAL.name));
    expect(screen.getByLabelText("Credential secret")).toHaveValue("");
    expect(screen.getByRole("button", { name: "Delete credential" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    expect(submitted[0]).not.toHaveProperty("secret");
    expect(ifMatch).toBe(`"${UPSTREAM_CREDENTIAL.updated_at}"`);
    await waitFor(() => expect(screen.getByRole("button", { name: "Save credential" })).toBeEnabled());
    await user.type(screen.getByLabelText("Credential secret"), "rotated-test-secret");
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(submitted[1]?.secret).toBe("rotated-test-secret"));
  });

  it("reloads a conflicting credential edit", async () => {
    let reads = 0;
    server.use(
      http.get("/console/v1/routing/upstream-credentials/:id", () => {
        reads += 1;
        return HttpResponse.json(UPSTREAM_CREDENTIAL_DETAIL, { headers: { ETag: `"${UPSTREAM_CREDENTIAL.updated_at}"` } });
      }),
      http.put("/console/v1/routing/upstream-credentials/:id", () => HttpResponse.json({ error: "conflict" }, { status: 409 })),
    );
    const user = userEvent.setup();
    renderAt(UPSTREAM_CREDENTIAL.id);
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(UPSTREAM_CREDENTIAL.name));
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(reads).toBeGreaterThan(1));
    expect(await screen.findByText("This credential was changed elsewhere or is still in use. Reloading.")).toBeInTheDocument();
  });

  it("does not expose ordinary credential editing for provider-managed identities", async () => {
    server.use(http.get("/console/v1/routing/upstream-credentials/:id", () => HttpResponse.json({
      ...UPSTREAM_CREDENTIAL_DETAIL, kind: "codex_oauth", provider_managed: true, secret: null, allowed_base_urls: [],
    }, { headers: { ETag: `"${UPSTREAM_CREDENTIAL.updated_at}"` } })));
    renderAt(UPSTREAM_CREDENTIAL.id);
    expect(await screen.findByText("This identity is managed by the Codex connector. Use its provider page to change or delete it.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save credential" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete credential" })).not.toBeInTheDocument();
  });
});
