import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CONNECTOR_PLUGINS, UPSTREAM_CREDENTIAL, UPSTREAM_CREDENTIAL_DETAIL } from "@/test/fixtures";
import type { UpstreamCredentialInput } from "@/api/types";

function renderAt(id: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", `/admin/routing/upstream-credentials/${id}`);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

describe("upstream credential management", () => {
  it("assigns a static credential to a registered plugin without offering Codex impersonation", async () => {
    let submitted: UpstreamCredentialInput | undefined;
    server.use(
      http.get("/console/v1/system/connectors", () => HttpResponse.json([
        ...CONNECTOR_PLUGINS,
        { id: "acme", version: "1.0.0", abi_version: 1, built_in: false, operations: ["responses"] },
      ])),
      http.post("/console/v1/routing/upstream-credentials", async ({ request }) => {
        submitted = await request.json() as UpstreamCredentialInput;
        return HttpResponse.json({ id: UPSTREAM_CREDENTIAL.id }, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAt("new");
    await user.type(await screen.findByLabelText("Name"), "Plugin credential");
    await user.type(screen.getByLabelText("Credential secret"), "test-secret");
    await user.type(screen.getByLabelText("Allowed Base URLs"), "https://api.example.test");
    await user.click(screen.getByRole("combobox", { name: "Connector" }));
    expect(screen.queryByRole("option", { name: "Codex" })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("option", { name: "acme" }));
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(submitted?.connector_kind).toBe("acme"));
    expect(submitted?.kind).toBe("bearer");
  });
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
      name: "Reusable identity", kind: "bearer", connector_kind: "general", header_name: null,
      secret: "test-new-secret", allowed_base_urls: ["https://api.example.test"], enabled: true,
    }));
  });

  it("creates from a plugin tab without reselecting the connector", async () => {
    let submitted: UpstreamCredentialInput | undefined;
    server.use(
      http.get("/console/v1/system/connectors", () => HttpResponse.json([
        ...CONNECTOR_PLUGINS,
        { id: "acme", version: "1.0.0", abi_version: 1, built_in: false, operations: ["responses"] },
      ])),
      http.post("/console/v1/routing/upstream-credentials", async ({ request }) => {
        submitted = await request.json() as UpstreamCredentialInput;
        return HttpResponse.json({ id: UPSTREAM_CREDENTIAL.id, correlation_id: UPSTREAM_CREDENTIAL.id }, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAt("?connector=acme");
    await user.click(await screen.findByRole("link", { name: "New credential" }));
    expect(await screen.findByRole("combobox", { name: "Connector" })).toHaveTextContent("acme");
    await user.type(screen.getByLabelText("Name"), "Scoped to plugin");
    await user.type(screen.getByLabelText("Credential secret"), "test-secret");
    await user.type(screen.getByLabelText("Allowed Base URLs"), "https://api.example.test");
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(submitted?.connector_kind).toBe("acme"));
  });

  it.each(["uninstalled", "codex"])("requires a valid static connector for a %s create link", async (requested) => {
    const user = userEvent.setup();
    renderAt(`new?connector=${requested}`);
    expect(await screen.findByText("Choose an available connector.")).toBeVisible();
    const save = screen.getByRole("button", { name: "Save credential" });
    expect(save).toBeDisabled();
    await user.click(screen.getByRole("combobox", { name: "Connector" }));
    await user.click(await screen.findByRole("option", { name: "General" }));
    expect(save).toBeEnabled();
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
    const writes: { ifMatch: string | null; name: string }[] = [];
    const editedElsewhere = { ...UPSTREAM_CREDENTIAL_DETAIL, name: "New upstream identity", updated_at: "2026-08-02T00:00:00Z" };
    const path = `/console/v1/routing/upstream-credentials/${UPSTREAM_CREDENTIAL.id}`;
    server.use(
      http.get(path, () => {
        const current = writes.length ? editedElsewhere : UPSTREAM_CREDENTIAL_DETAIL;
        return HttpResponse.json(current, { headers: { ETag: `"${current.updated_at}"` } });
      }),
      http.put(path, async ({ request }) => {
        const { name } = await request.json() as { name: string };
        const ifMatch = request.headers.get("if-match");
        writes.push({ ifMatch, name });
        return ifMatch === `"${editedElsewhere.updated_at}"`
          ? HttpResponse.json({ id: UPSTREAM_CREDENTIAL.id })
          : HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAt(UPSTREAM_CREDENTIAL.id);
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(UPSTREAM_CREDENTIAL.name));
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    expect(await screen.findByText("This credential was changed elsewhere or is still in use. Reloading.")).toBeInTheDocument();
    expect(await screen.findByDisplayValue(editedElsewhere.name)).toBeVisible();
    expect(writes).toEqual([{ ifMatch: `"${UPSTREAM_CREDENTIAL.updated_at}"`, name: UPSTREAM_CREDENTIAL.name }]);
    await user.click(screen.getByRole("button", { name: "Save credential" }));
    await waitFor(() => expect(writes).toHaveLength(2));
    expect(writes[1]).toEqual({ ifMatch: `"${editedElsewhere.updated_at}"`, name: editedElsewhere.name });
  });

  it("does not expose ordinary credential editing for provider-managed identities", async () => {
    server.use(http.get("/console/v1/routing/upstream-credentials/:id", () => HttpResponse.json({
      ...UPSTREAM_CREDENTIAL_DETAIL, connector_kind: "codex", kind: "codex_oauth", provider_managed: true, secret: null,
    }, { headers: { ETag: `"${UPSTREAM_CREDENTIAL.updated_at}"` } })));
    renderAt(UPSTREAM_CREDENTIAL.id);
    await waitFor(() => expect(screen.getByRole("link", { name: "Back to credentials" }))
      .toHaveAttribute("href", "/admin/routing/upstream-credentials?connector=codex"));
    expect(screen.queryByRole("button", { name: "Save credential" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete credential" })).not.toBeInTheDocument();
  });
});
