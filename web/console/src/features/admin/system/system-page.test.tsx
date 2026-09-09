import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { SYSTEM_SETTINGS } from "@/test/fixtures";

function renderApp(section = "upstream") {
  window.history.replaceState({}, "", `/admin/system/${section}`);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("SystemPage", () => {
  it("redirects the old settings URL and opens only the selected category", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp("");
    expect(await screen.findByRole("heading", { name: "System settings · General settings" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/admin/system/general");
    const menu = screen.getByRole("button", { name: "System settings" });
    expect(menu).toHaveAttribute("aria-expanded", "true");
    expect(screen.queryByLabelText("Connect timeout (seconds)")).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "MCP Servers" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: "Codex" }));
    const originator = await screen.findByLabelText("Codex originator");
    await user.clear(originator);
    await user.type(originator, "unsaved-draft");
    await user.click(screen.getByRole("link", { name: "Upstream timeouts" }));
    expect(await screen.findByLabelText("Connect timeout (seconds)")).toBeInTheDocument();
    expect(screen.queryByLabelText("Codex originator")).not.toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "Codex" }));
    expect(await screen.findByLabelText("Codex originator")).toHaveValue(SYSTEM_SETTINGS.codex.originator);
  });

  it("reloads the complete settings and ETag after an edit conflict", async () => {
    seedAuthenticatedSession();
    let conflict = false;
    let savedEtag: string | null = null;
    server.use(
      http.get("/console/v1/system/settings", () =>
        HttpResponse.json(
          { ...SYSTEM_SETTINGS, upstream: { ...SYSTEM_SETTINGS.upstream, connect_timeout_seconds: conflict ? 14 : 10 } },
          { headers: { ETag: conflict ? '"new-version"' : '"old-version"' } },
        ),
      ),
      http.put("/console/v1/system/settings", ({ request }) => {
        savedEtag = request.headers.get("if-match");
        conflict = true;
        return HttpResponse.json({ error: "conflict", message: "Changed elsewhere" }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderApp();
    const timeout = await screen.findByLabelText("Connect timeout (seconds)");
    await user.clear(timeout);
    await user.type(timeout, "12");
    await user.click(screen.getByRole("button", { name: "Save system settings" }));
    expect(savedEtag).toBe('"old-version"');
    await waitFor(() => expect(timeout).toHaveValue(14));
    await user.click(screen.getByRole("button", { name: "Save system settings" }));
    expect(savedEtag).toBe('"new-version"');
  });

  it("persists database-backed forwarding settings with its ETag", async () => {
    seedAuthenticatedSession();
    let received: unknown;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/system/settings", async ({ request }) => {
        received = await request.json();
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({
          id: "00000000-0000-0000-0000-0000000000f1",
          correlation_id: "11111111-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderApp();

    const connectTimeout = await screen.findByLabelText("Connect timeout (seconds)");
    await user.clear(connectTimeout);
    await user.type(connectTimeout, "12");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(received).toEqual({
      ...Object.fromEntries(Object.entries(SYSTEM_SETTINGS).filter(([key]) => key !== "updated_at")),
      upstream: { ...SYSTEM_SETTINGS.upstream, connect_timeout_seconds: 12 },
    });
    expect(screen.queryByLabelText("Codex originator")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Maximum cache entries")).not.toBeInTheDocument();
    expect(ifMatch).toBe('"2026-01-02T00:00:00.000Z"');
    expect(await screen.findByText("System settings saved and applied.")).toBeInTheDocument();
  });

  it("requires the response-header timeout to exceed the connect timeout", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp();

    const responseHeaderTimeout = await screen.findByLabelText(
      "Response header timeout (seconds)",
    );
    await user.clear(responseHeaderTimeout);
    await user.type(responseHeaderTimeout, "10");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(
      await screen.findByText("Response header timeout must exceed connect timeout."),
    ).toBeInTheDocument();
  });


  it("requires the Images response-header timeout to exceed the connect timeout", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp();

    const imagesResponseHeaderTimeout = await screen.findByLabelText(
      "Images response header timeout (seconds)",
    );
    await user.clear(imagesResponseHeaderTimeout);
    await user.type(imagesResponseHeaderTimeout, "10");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(
      await screen.findByText("Images response header timeout must exceed connect timeout."),
    ).toBeInTheDocument();
  });

  it("requires the web-search response-header timeout to exceed the connect timeout", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp();

    const responseHeaderTimeout = await screen.findByLabelText(
      "Web search response header timeout (seconds)",
    );
    await user.clear(responseHeaderTimeout);
    await user.type(responseHeaderTimeout, "10");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(
      await screen.findByText(
        "Web search response header timeout must exceed connect timeout.",
      ),
    ).toBeInTheDocument();
  });

  it("bounds automatic retries after the initial attempt", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp("reliability");

    const maximumRetries = await screen.findByLabelText("Maximum retries");
    await user.clear(maximumRetries);
    await user.type(maximumRetries, "11");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(
      await screen.findByText("Maximum retries must be between 1 and 10."),
    ).toBeInTheDocument();
  });

  it("requires a synthetic HTTPS Codex Git remote", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp("codex");

    const gitRemote = await screen.findByLabelText("Synthetic Git origin");
    await user.clear(gitRemote);
    await user.type(gitRemote, "http://github.com/private/repo");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(
      await screen.findByText(
        "Enter a valid HTTPS repository URL without credentials, query, or fragment.",
      ),
    ).toBeInTheDocument();
  });

  it("shows valid affinity cache counts and clears one rule", async () => {
    seedAuthenticatedSession();
    let clearedRule: string | null = null;
    server.use(
      http.get("/console/v1/system/settings", () =>
        HttpResponse.json(
          {
            ...SYSTEM_SETTINGS,
            session_affinity: {
              ...SYSTEM_SETTINGS.session_affinity,
              enabled: true,
              rules: [
                {
                  name: "codex-responses",
                  enabled: true,
                  api_formats: ["open_ai_responses"],
                  model_regex: ["^gpt-.*$"],
                  key_sources: [
                    { type: "json_pointer", pointer: "/prompt_cache_key" },
                  ],
                  value_regex: null,
                  ttl_seconds: null,
                },
              ],
            },
          },
          { headers: { ETag: `"${SYSTEM_SETTINGS.updated_at}"` } },
        ),
      ),
      http.get("/console/v1/system/session-affinity/cache", () =>
        HttpResponse.json({
          enabled: true,
          max_entries: 100_000,
          total_entries: 3,
          rules: [{ name: "codex-responses", entries: 3 }],
        }),
      ),
      http.delete("/console/v1/system/session-affinity/cache", ({ request }) => {
        clearedRule = new URL(request.url).searchParams.get("rule_name");
        return HttpResponse.json({
          cleared_entries: 3,
          cache: {
            enabled: true,
            max_entries: 100_000,
            total_entries: 0,
            rules: [{ name: "codex-responses", entries: 0 }],
          },
        });
      }),
    );
    const user = userEvent.setup();
    renderApp("affinity");

    const rule = await screen.findByText("codex-responses");
    const row = rule.closest("tr");
    expect(row).not.toBeNull();
    expect(
      await within(row as HTMLElement).findByText("3"),
    ).toBeInTheDocument();

    await user.click(
      within(row as HTMLElement).getByRole("button", {
        name: "Clear cache for codex-responses",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Clear cache" }));

    await waitFor(() => expect(clearedRule).toBe("codex-responses"));
    expect(await screen.findByText("Cleared 3 cached entries.")).toBeInTheDocument();
  });
});
