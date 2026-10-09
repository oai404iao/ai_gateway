import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { ADMIN_LOGIN_RESPONSE, PLUGIN, PLUGIN_JOB, PLUGIN_SETTINGS } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";
import type { PluginSettingsInput } from "@/api/types";

function renderPage(path = "/admin/plugins/example") {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

async function confirm(user: ReturnType<typeof userEvent.setup>, label: string) {
  const dialog = await screen.findByRole("dialog", { name: "Authorize plugin operation" });
  await user.type(within(dialog).getByLabelText("Current password"), "administrator-password");
  await user.click(within(dialog).getByRole("button", { name: label }));
}

describe("Plugin management", () => {
  it("renders a non-provider-specific descriptor and preserves seeded boolean and enum values", async () => {
    let body: PluginSettingsInput | undefined;
    let etag: string | null = null;
    let token: string | null = null;
    server.use(http.put("/console/v1/plugins/example/settings", async ({ request }) => {
      body = await request.json() as PluginSettingsInput;
      etag = request.headers.get("if-match");
      token = request.headers.get("x-plugin-authorization");
      return HttpResponse.json({ id: "example", correlation_id: "saved" });
    }));
    const user = userEvent.setup();
    renderPage();
    expect(await screen.findByLabelText("Client label")).toHaveValue("configured-client");
    expect(screen.getByLabelText("Attempt limit")).toHaveValue(3);
    expect(screen.getByRole("switch", { name: "Compact payload" })).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "Protocol mode" })).toHaveTextContent("Extended");
    await user.clear(screen.getByLabelText("Client label"));
    await user.type(screen.getByLabelText("Client label"), "new-client");
    await user.click(screen.getByRole("button", { name: "Save plugin settings" }));
    expect(body).toBeUndefined();
    await confirm(user, "Save plugin settings");
    await waitFor(() => expect(body).toEqual({ schema_version: 1, values: { ...PLUGIN_SETTINGS.values, client_label: "new-client" } }));
    expect(etag).toBe('"plugin-3"');
    expect(token).toBe("plugin-test-authorization");
    expect(await screen.findByText("Plugin settings saved and applied.")).toBeInTheDocument();
    expect(JSON.stringify(localStorage)).not.toContain("plugin-test-authorization");
    expect(JSON.stringify(sessionStorage)).not.toContain("administrator-password");
  });

  it("rejects a required field before requesting native authorization", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.clear(await screen.findByLabelText("Attempt limit"));
    await user.click(screen.getByRole("button", { name: "Save plugin settings" }));
    expect(await screen.findByText("This setting is required.")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("leaves empty-string business validation to the plugin rather than inventing a minimum length", async () => {
    let body: PluginSettingsInput | undefined;
    server.use(http.put("/console/v1/plugins/example/settings", async ({ request }) => {
      body = await request.json() as PluginSettingsInput;
      return HttpResponse.json({ id: "example", correlation_id: "saved" });
    }));
    const user = userEvent.setup();
    renderPage();
    await user.clear(await screen.findByLabelText("Client label"));
    await user.click(screen.getByRole("button", { name: "Save plugin settings" }));
    await confirm(user, "Save plugin settings");
    await waitFor(() => expect(body?.values.client_label).toBe(""));
  });

  it("never mutates after failed password verification and clears the password", async () => {
    let writes = 0;
    server.use(
      http.post("/console/v1/plugins/reauth", () => HttpResponse.json({ error: "forbidden" }, { status: 403 })),
      http.put("/console/v1/plugins/example/state", () => { writes++; return HttpResponse.json({}); }),
    );
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole("button", { name: "Disable plugin" }));
    await confirm(user, "Disable plugin");
    expect(await screen.findByText("Console rejected the request (forbidden).")).toBeInTheDocument();
    expect(screen.getByLabelText("Current password")).toHaveValue("");
    expect(writes).toBe(0);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("does not refresh and replay a rejected single-use authorization token", async () => {
    let writes = 0;
    let refreshes = 0;
    server.use(
      http.put("/console/v1/plugins/example/state", () => {
        writes++;
        return HttpResponse.json({ error: "invalid_credentials" }, { status: 401 });
      }),
      http.post("/console/v1/auth/refresh", () => {
        refreshes++;
        return HttpResponse.json(ADMIN_LOGIN_RESPONSE);
      }),
    );
    const user = userEvent.setup();
    renderPage();
    const disable = await screen.findByRole("button", { name: "Disable plugin" });
    const initialRefreshes = refreshes;
    await user.click(disable);
    await confirm(user, "Disable plugin");
    expect(await screen.findByText("Console rejected the request (invalid_credentials).")).toBeInTheDocument();
    expect(writes).toBe(1);
    expect(refreshes).toBe(initialRefreshes);
  });

  it("reloads values and ETags after a conflict rather than retrying stale state", async () => {
    let conflict = false;
    const etags: (string | null)[] = [];
    server.use(
      http.get("/console/v1/plugins/example/settings", () => HttpResponse.json({
        ...PLUGIN_SETTINGS, values: { ...PLUGIN_SETTINGS.values, client_label: conflict ? "changed-elsewhere" : "configured-client" },
      }, { headers: { ETag: conflict ? '"plugin-4"' : '"plugin-3"' } })),
      http.put("/console/v1/plugins/example/settings", ({ request }) => {
        etags.push(request.headers.get("if-match"));
        if (!conflict) {
          conflict = true;
          return HttpResponse.json({ error: "conflict" }, { status: 409 });
        }
        return HttpResponse.json({ id: "example", correlation_id: "saved" });
      }),
    );
    const user = userEvent.setup();
    renderPage();
    await screen.findByLabelText("Client label");
    await user.click(screen.getByRole("button", { name: "Save plugin settings" }));
    await confirm(user, "Save plugin settings");
    await waitFor(() => expect(screen.getByLabelText("Client label")).toHaveValue("changed-elsewhere"));
    await user.click(screen.getByRole("button", { name: "Save plugin settings" }));
    await confirm(user, "Save plugin settings");
    await waitFor(() => expect(etags).toEqual(['"plugin-3"', '"plugin-4"']));
  });

  it("keeps the built-in connector read-only without fetching plugin settings", async () => {
    let settingsReads = 0;
    server.use(http.get("/console/v1/plugins/general/settings", () => {
      settingsReads++;
      return HttpResponse.json(PLUGIN_SETTINGS);
    }));
    renderPage("/admin/plugins/general");
    expect(await screen.findByText("Built-in connector. Lifecycle and plugin settings are not editable.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disable plugin" })).not.toBeInTheDocument();
    expect(settingsReads).toBe(0);
  });

  it("switches versions only after explicit reauthentication with the detail ETag", async () => {
    let body: unknown;
    let etag: string | null = null;
    server.use(http.put("/console/v1/plugins/example/state", async ({ request }) => {
      body = await request.json();
      etag = request.headers.get("if-match");
      return HttpResponse.json({ id: "example", correlation_id: "upgraded" });
    }));
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole("combobox", { name: "Installed version" }));
    await user.click(await screen.findByRole("option", { name: /1.1.0/ }));
    await user.click(screen.getByRole("button", { name: "Enable selected version" }));
    await confirm(user, "Enable selected version");
    await waitFor(() => expect(body).toEqual({ enabled: true, artifact_digest: "b".repeat(64) }));
    expect(etag).toBe('"plugin-3"');
  });

  it("uploads raw package bytes and follows the asynchronous install job without auto-enabling", async () => {
    let contentType: string | null = null;
    let raw: string | undefined;
    let token: string | null = null;
    server.use(http.post("/console/v1/plugins/install", async ({ request }) => {
      contentType = request.headers.get("content-type");
      token = request.headers.get("x-plugin-authorization");
      raw = await request.text();
      return HttpResponse.json({ ...PLUGIN_JOB, status: "queued" }, { status: 202 });
    }));
    const user = userEvent.setup();
    renderPage("/admin/plugins");
    expect(await screen.findByRole("link", { name: PLUGIN.id })).toBeInTheDocument();
    const { File: NativeFile } = await vi.importActual<{ File: typeof File }>("node:buffer");
    await user.upload(screen.getByLabelText("Plugin package"), new NativeFile(["package-bytes"], "plugin.tar.gz", { type: "application/gzip" }));
    await user.click(screen.getByRole("button", { name: "Install plugin" }));
    await confirm(user, "Install plugin");
    expect(await screen.findByText("succeeded")).toBeInTheDocument();
    expect(raw).toBe("package-bytes");
    expect(contentType).toBe("application/octet-stream");
    expect(token).toBe("plugin-test-authorization");
    expect(window.location.search).toContain(PLUGIN_JOB.id);
  });

  it("shows discovery job failures and preserves the installed plugin inventory", async () => {
    server.use(
      http.post("/console/v1/plugins/discover", () => HttpResponse.json({ ...PLUGIN_JOB, operation: "discover", status: "queued" }, { status: 202 })),
      http.get("/console/v1/plugins/jobs/:id", () => HttpResponse.json({ ...PLUGIN_JOB, operation: "discover", status: "failed", error_code: "invalid_package" })),
    );
    const user = userEvent.setup();
    renderPage("/admin/plugins");
    await user.click(await screen.findByRole("button", { name: "Discover plugins" }));
    await confirm(user, "Discover plugins");
    expect(await screen.findByText("Plugin operation failed (invalid_package).")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "general" })).toBeInTheDocument();
  });
});
