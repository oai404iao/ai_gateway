import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { SYSTEM_SETTINGS } from "@/test/fixtures";

function renderApp() {
  window.history.replaceState({}, "", "/admin/system");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("SystemPage", () => {
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
    expect(
      connectTimeout.closest('[data-slot="system-settings-columns"]'),
    ).toHaveClass("xl:grid-cols-2");
    expect(
      screen
        .getByLabelText("Maximum cache entries")
        .closest('[data-slot="field-group"]'),
    ).toHaveClass("xl:grid-cols-2");
    await user.clear(connectTimeout);
    await user.type(connectTimeout, "12");
    await user.click(screen.getByRole("button", { name: "Add Codex template" }));
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(received).toEqual({
      api_hosts: ["https://api.example.test/v1"],
      upstream: {
        connect_timeout_seconds: 12,
        response_header_timeout_seconds: 30,
        images_response_header_timeout_seconds: 300,
        stream_idle_timeout_seconds: 90,
      },
      request_retry: {
        enabled: true,
        max_retries: 1,
      },
      passive_health: {
        connection_failure_threshold: 3,
        cooldown_seconds: 30,
      },
      automatic_disable: {
        enabled: true,
        error_status_codes: [429, 500],
        error_message_keywords: ["quota exceeded"],
      },
      scheduled_testing: {
        mode: "global",
        auto_recover: true,
        interval_minutes: 5,
        prompt: "reply '1'",
      },
      session_affinity: {
        enabled: false,
        max_entries: 100000,
        default_ttl_seconds: 3600,
        rules: [
          {
            name: "codex-responses",
            enabled: true,
            api_formats: ["open_ai_responses"],
            model_regex: ["^gpt-.*$"],
            key_sources: [
              { type: "json_pointer", pointer: "/prompt_cache_key" },
              { type: "request_header", name: "session_id" },
              { type: "request_header", name: "thread_id" },
            ],
            value_regex: null,
            ttl_seconds: null,
          },
        ],
      },
      websocket: {
        enabled: false,
        max_idle_connections: 128,
        idle_timeout_seconds: 300,
        max_connection_age_seconds: 3300,
      },
    });
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

  it("bounds automatic retries after the initial attempt", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp();

    const maximumRetries = await screen.findByLabelText("Maximum retries");
    await user.clear(maximumRetries);
    await user.type(maximumRetries, "11");
    await user.click(screen.getByRole("button", { name: /save system settings/i }));

    expect(
      await screen.findByText("Maximum retries must be between 1 and 10."),
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
    renderApp();

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
