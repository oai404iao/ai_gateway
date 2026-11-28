import { describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  ADMIN_API_KEY,
  CHANNEL,
  CHANNEL_GROUP,
  CONTROL_PLANE_USER,
  OWN_API_KEY,
  REQUEST_LOG,
} from "@/test/fixtures";
import { server, seedAuthenticatedSession, seedUserSession } from "@/test/msw";

async function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
  await act(() => vi.dynamicImportSettled());
}

describe("RequestLogsView", () => {
  it("compacts usage in the table but keeps the exact token breakdown available", async () => {
    seedUserSession();
    server.use(http.get("/console/v1/me/request-logs", () => HttpResponse.json({
      items: [{ ...REQUEST_LOG, input_tokens: 1_234_569, cached_input_tokens: 2, cost_amount: "0.0000008" }],
      next_cursor: null,
    })));
    await renderAppAt("/usage/request-logs");
    expect(await screen.findByLabelText("Uncached input: 1,234,567")).toHaveTextContent("1.23M");
    expect(screen.getByText("<0.00001 USD")).toBeVisible();
  });

  it("keeps the personal request-log table owner-scoped even for administrators", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    const catalogOnlyModel = "priced/not-in-history";
    const log = { ...REQUEST_LOG, upstream_model: "upstream-only" };
    const adminCatalogRequests: string[] = [];
    server.use(
      http.get(/\/console\/v1\/(?:models|routing\/profiles|routing\/operation-rules)$/, ({ request }) => {
        adminCatalogRequests.push(request.url);
        return HttpResponse.json([]);
      }),
      http.get("/console/v1/model-options", () => HttpResponse.json([catalogOnlyModel])),
      http.get("/console/v1/me/request-logs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json({ items: [log], next_cursor: null });
      }),
    );

    const user = userEvent.setup();
    await renderAppAt("/usage/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    expect(
      screen.getAllByRole("columnheader").map((header) => header.textContent),
    ).toEqual([
      "Started",
      "Model",
      "Operation / protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "View details",
    ]);
    expect(screen.getByText("Chat Completions")).toBeInTheDocument();
    const modelCell = screen.getByRole("cell", { name: /Requested model:/ });
    expect(modelCell).toHaveTextContent(`Requested model: ${log.client_model}`);
    expect(modelCell).toHaveTextContent(`Upstream model: ${log.upstream_model}`);
    expect(screen.getByText("SSE")).toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.name)).not.toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.id)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Uncached input: 10")).toHaveTextContent("10");
    expect(screen.getByLabelText("Cached input: 2")).toHaveTextContent("2");
    expect(screen.getByLabelText("Output tokens: 4")).toHaveTextContent("4");
    expect(screen.getByLabelText("Reasoning tokens: 1")).toHaveTextContent("1");
    expect(screen.getByLabelText("Reasoning effort: High")).toHaveTextContent("High");
    expect(screen.getByLabelText("Fast mode")).toHaveTextContent("Fast");
    expect(screen.queryByLabelText("Peak pricing")).not.toBeInTheDocument();
    expect(screen.getByLabelText("TTFT: 100 ms")).toBeVisible();
    expect(screen.getByLabelText("Total duration: 1 s")).toBeVisible();
    expect(screen.getByLabelText("TPS: 4.44 tok/s")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Filters (0)" }));
    const apiKey = await screen.findByRole("combobox", { name: "API key" });
    await user.click(apiKey);
    await user.click(
      await screen.findByRole("option", {
        name: OWN_API_KEY.name,
      }),
    );

    const model = screen.getByRole("combobox", { name: "Requested model" });
    expect(model).toHaveAccessibleDescription("Choose a priced model or enter the exact model ID from the request.");
    await user.click(model);
    const catalogOption = await screen.findByRole("option", { name: catalogOnlyModel });
    expect(screen.queryByRole("option", { name: log.client_model })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: log.upstream_model })).not.toBeInTheDocument();
    await user.click(catalogOption);

    const operation = screen.getByRole("combobox", { name: "Operation" });
    await user.click(operation);
    await user.click(
      await screen.findByRole("option", { name: "Chat Completions" }),
    );
    await user.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(
        queries.some(
          (query) =>
            query.get("api_key_id") === OWN_API_KEY.id &&
            query.get("model") === catalogOnlyModel &&
            query.get("api_operation") === REQUEST_LOG.api_operation,
        ),
      ).toBe(true);
    });
    expect(adminCatalogRequests).toEqual([]);
  });

  it("hides request-mode badges when the client did not enable them", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/me/request-logs", () =>
        HttpResponse.json({
          items: [
            {
              ...REQUEST_LOG,
              reasoning_effort: null,
              fast_mode: false,
            },
          ],
          next_cursor: null,
        }),
      ),
    );

    await renderAppAt("/usage/request-logs");

    await screen.findByText(REQUEST_LOG.client_model);
    expect(screen.getByRole("cell", { name: /Requested & upstream:/ })).toHaveTextContent(REQUEST_LOG.client_model);
    expect(screen.queryByLabelText(/Reasoning effort:/)).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Fast mode")).not.toBeInTheDocument();
  });

  it("adds user and channel names only to the system request-log table", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    const historicalModel = "retired/requested-model";
    server.use(
      http.get("/console/v1/model-options", () => HttpResponse.json(["priced/catalog-option"])),
      http.get("/console/v1/request-logs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json({ items: [REQUEST_LOG], next_cursor: null });
      }),
    );

    const user = userEvent.setup();
    await renderAppAt("/admin/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    expect(
      screen.getAllByRole("columnheader").map((header) => header.textContent),
    ).toEqual([
      "Started",
      "Model",
      "Operation / protocol",
      "Channel group",
      "Channel",
      "User",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "View details",
    ]);
    expect(screen.getByText(CHANNEL_GROUP.name)).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: CHANNEL.name })).toBeInTheDocument();
    expect(
      screen.getByRole("cell", { name: CONTROL_PLANE_USER.display_name }),
    ).toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.id)).not.toBeInTheDocument();
    expect(screen.queryByText(CONTROL_PLANE_USER.id)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Filters (0)" }));
    const userSelect = await screen.findByRole("combobox", { name: "User" });
    await user.click(userSelect);
    await user.click(
      await screen.findByRole("option", {
        name: `${CONTROL_PLANE_USER.display_name} · ${CONTROL_PLANE_USER.email}`,
      }),
    );

    const apiKeySelect = screen.getByRole("combobox", { name: "API key" });
    await user.click(apiKeySelect);
    await user.click(
      await screen.findByRole("option", {
        name: ADMIN_API_KEY.name,
      }),
    );

    const modelSelect = screen.getByRole("combobox", { name: "Requested model" });
    await user.type(modelSelect, historicalModel);
    await user.tab();
    expect(modelSelect).toHaveValue(historicalModel);
    await user.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(
        queries.some(
          (query) =>
            query.get("user_id") === CONTROL_PLANE_USER.id &&
            query.get("api_key_id") === ADMIN_API_KEY.id &&
            query.get("model") === historicalModel,
        ),
      ).toBe(true);
    });
  });

  it("shows the full long routing name in a tooltip on keyboard focus", async () => {
    seedAuthenticatedSession();
    const longGroupName =
      "channel-group-name-that-is-long-enough-to-overflow-the-column";
    const longChannelName =
      "channel-name-that-is-long-enough-to-overflow-the-column";
    const longUserName =
      "user-name-that-is-long-enough-to-overflow-the-column";
    server.use(
      http.get("/console/v1/request-logs", () =>
        HttpResponse.json({
          items: [
            {
              ...REQUEST_LOG,
              channel_group_name: longGroupName,
              channel_name: longChannelName,
              user_name: longUserName,
            },
          ],
          next_cursor: null,
        }),
      ),
    );

    await renderAppAt("/admin/request-logs");

    const groupName = await screen.findByText(longGroupName);
    expect(screen.getByRole("cell", { name: longChannelName })).toBeVisible();
    expect(screen.getByRole("cell", { name: longUserName })).toBeVisible();

    const user = userEvent.setup();
    act(() => groupName.focus());
    await user.tab({ shift: true });
    await user.tab();
    expect(groupName).toHaveFocus();
    await waitFor(() => {
      const fullValue = screen.getAllByText(longGroupName).find((element) => element !== groupName);
      expect(fullValue).toBeVisible();
    });
  });

  it("loads administrator request-log details from the admin detail endpoint", async () => {
    seedAuthenticatedSession();
    const log = {
      ...REQUEST_LOG,
      user_name: "System Request Owner",
      request_source: "scheduled_test",
      api_format: "open_ai_responses",
      api_operation: "responses",
      request_protocol: "websocket",
      client_model: "detail-model",
      upstream_model: "upstream-detail-model",
      model_rule_id: null,
      channel_group_id: CHANNEL_GROUP.id,
      channel_group_name: CHANNEL_GROUP.name,
      channel_id: CHANNEL.id,
      channel_name: CHANNEL.name,
      outcome: "failed",
      response_status_code: 200,
          ttft_ms: 100,
      total_duration_ms: 1_000,
      input_tokens: 12,
      cached_input_tokens: 2,
      cache_write_tokens: 0,
      output_tokens: 4,
      reasoning_tokens: 1,
      cost_amount: "0.0001",
      error_code: "provider_error",
      error_summary: "Upstream quota exhausted.\nTry another channel.",
      billed_at: "2026-07-21T06:00:02Z",
    } as const;
    let detailRequests = 0;
    server.use(
      http.get("/console/v1/request-logs", () => HttpResponse.json({ items: [log], next_cursor: null })),
      http.get("/console/v1/request-logs/:id", ({ params }) => {
        expect(params.id).toBe(log.id);
        detailRequests += 1;
        return HttpResponse.json(log);
      }),
    );

    const user = userEvent.setup();
    await renderAppAt("/admin/request-logs");

    const model = await screen.findByText("detail-model");
    expect(screen.queryByText(log.id.slice(0, 8))).not.toBeInTheDocument();
    await user.click(model);
    expect(await screen.findByLabelText("Request ID")).toHaveTextContent(log.id);
    await screen.findByText("Completed", { selector: "dt" });
    const detailModel = within(screen.getByRole("complementary", { name: "Request log" })).getByText("Model", { selector: "dt" }).parentElement;
    expect(detailModel).toHaveTextContent(`Requested model: ${log.client_model}`);
    expect(detailModel).toHaveTextContent(`Upstream model: ${log.upstream_model}`);
    expect([...document.querySelectorAll("dt")].map((label) => label.textContent)).toEqual([
      "Started",
      "Model",
      "Operation",
      "Protocol",
      "Channel group",
      "Channel",
      "User",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "HTTP",
      "Error code",
      "Error detail",
      "Completed",
    ]);
    expect(
      await screen.findByText("provider_error", { selector: "dd" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Responses", { selector: "dd" })).toBeInTheDocument();
    expect(await screen.findByText("WebSocket", { selector: "dd" })).toBeInTheDocument();
    const channelLabel = await screen.findByText("Channel", { selector: "dt" });
    expect(channelLabel.parentElement).toHaveTextContent(CHANNEL.name);
    const userLabel = await screen.findByText("User", { selector: "dt" });
    expect(userLabel.parentElement).toHaveTextContent("System Request Owner");
    const errorMessageLabel = await screen.findByText("Error detail", { selector: "dt" });
    expect(errorMessageLabel.parentElement).toHaveTextContent(
      "Upstream quota exhausted. Try another channel.",
    );
    expect(screen.queryByText(CHANNEL.id)).not.toBeInTheDocument();
    expect(screen.queryByText(log.user_id)).not.toBeInTheDocument();
    expect(detailRequests).toBe(1);
  });

  it("labels standalone web-search request details", async () => {
    seedAuthenticatedSession();
    const log = {
      ...REQUEST_LOG,
      id: "11111111-2222-4333-8444-666666666666",
      api_format: "open_ai_responses",
      api_operation: "web_search",
      request_protocol: "non_stream",
      client_model: "search-model",
    } as const;
    server.use(
      http.get("/console/v1/request-logs", () => HttpResponse.json({ items: [log], next_cursor: null })),
      http.get("/console/v1/request-logs/:id", () => HttpResponse.json(log)),
    );

    const user = userEvent.setup();
    await renderAppAt("/admin/request-logs");

    await user.click(await screen.findByText("search-model"));
    expect(
      await screen.findByText("Standalone web search", { selector: "dd" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Non-stream", { selector: "dd" })).toBeInTheDocument();
  });

  it("limits personal details to the standardized owner-visible fields", async () => {
    seedUserSession();
    const ownLog = {
      ...REQUEST_LOG,
      user_name: "unexpected-request-owner",
      channel_name: "unexpected-upstream-channel",
    };
    server.use(
      http.get("/console/v1/me/request-logs", () =>
        HttpResponse.json({ items: [ownLog], next_cursor: null }),
      ),
      http.get("/console/v1/me/request-logs/:id", () =>
        HttpResponse.json(ownLog),
      ),
    );

    const user = userEvent.setup();
    await renderAppAt("/usage/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    expect(
      screen.getAllByRole("columnheader").map((header) => header.textContent),
    ).toEqual([
      "Started",
      "Model",
      "Operation / protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "View details",
    ]);
    expect(await screen.findByText(CHANNEL_GROUP.name)).toBeInTheDocument();
    expect(screen.queryByText(ownLog.channel_name)).not.toBeInTheDocument();
    expect(screen.queryByText(ownLog.user_name)).not.toBeInTheDocument();

    await user.click(screen.getByText(REQUEST_LOG.client_model));
    await screen.findByText("Completed", { selector: "dt" });
    expect([...document.querySelectorAll("dt")].map((label) => label.textContent)).toEqual([
      "Started",
      "Model",
      "Operation",
      "Protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "HTTP",
      "Error code",
      "Error detail",
      "Completed",
    ]);
    const groupLabel = await screen.findByText("Channel group", { selector: "dt" });
    expect(groupLabel.parentElement).toHaveTextContent(CHANNEL_GROUP.name);
    expect(screen.queryByText("Channel", { selector: "dt" })).not.toBeInTheDocument();
    expect(screen.queryByText("User", { selector: "dt" })).not.toBeInTheDocument();
    const detail = within(screen.getByRole("complementary", { name: "Request log" }));
    expect(detail.queryByText(ownLog.channel_name)).not.toBeInTheDocument();
    expect(detail.queryByText(ownLog.user_name)).not.toBeInTheDocument();
  });

  it("shows the recorded upstream or its absence in compact rows and mobile details", async () => {
    vi.stubGlobal("innerWidth", 390);
    try {
      seedUserSession();
      const log = { ...REQUEST_LOG, upstream_model: "mobile-wire" };
      const unknown = { ...log, id: "f1111111-2222-4333-8444-555555555555", upstream_model: null };
      server.use(
        http.get("/console/v1/me/request-logs", () => HttpResponse.json({ items: [log, unknown], next_cursor: null })),
        http.get(`/console/v1/me/request-logs/${unknown.id}`, () => HttpResponse.json(unknown)),
      );
      await renderAppAt("/usage/request-logs");
      const row = await screen.findByRole("row", { name: /mobile-wire/ });
      expect(row).toHaveTextContent(`Requested model: ${log.client_model}`);
      expect(row).toHaveTextContent(`Upstream model: ${log.upstream_model}`);
      await userEvent.click(screen.getByText("Not recorded"));
      const detail = within(await screen.findByRole("dialog", { name: "Request log" }));
      const model = (await detail.findByText("Model", { selector: "dt" })).parentElement;
      expect(model).toHaveTextContent(`Requested model: ${unknown.client_model}`);
      expect(model).toHaveTextContent("Upstream model: Not recorded");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("opens an owner-scoped deep link outside the fetched slice and preserves its exact report bounds", async () => {
    seedAuthenticatedSession();
    const end = "2026-07-21T12:59:59.999999Z";
    let listQuery: URLSearchParams | undefined;
    server.use(
      http.get("/console/v1/me/request-logs", ({ request }) => {
        listQuery = new URL(request.url).searchParams;
        return HttpResponse.json({ items: [], next_cursor: null });
      }),
      http.get(`/console/v1/me/request-logs/${REQUEST_LOG.id}`, () => HttpResponse.json(REQUEST_LOG)),
    );
    await renderAppAt(`/usage/request-logs?${new URLSearchParams({ id: REQUEST_LOG.id, model: "report-model", started_before: end, api_format: "open_ai_responses", user_id: CONTROL_PLANE_USER.id })}`);
    const detail = within(await screen.findByRole("complementary", { name: "Request log" }));
    expect(await detail.findByText(REQUEST_LOG.client_model)).toBeVisible();
    expect(Object.fromEntries(listQuery!)).toEqual({limit: "50", model: "report-model", started_before: end, api_format: "open_ai_responses"});
    await userEvent.click(detail.getByRole("button", {name: "Close inspector"}));
    expect(new URLSearchParams(window.location.search).get("id")).toBeNull();
  });

  it("forwards AbortSignal to request log queries on unmount", async () => {
    seedAuthenticatedSession();
    let capturedSignal: AbortSignal | undefined;
    server.use(
      http.get("/console/v1/request-logs", ({ request }) => {
        capturedSignal = request.signal;
        return new Promise(() => {});
      }),
    );
    window.history.replaceState({}, "", "/admin/request-logs");
    const view = render(
      <AppProviders>
        <BrowserRouter>
          <AppRouter />
        </BrowserRouter>
      </AppProviders>,
    );
    await waitFor(() => expect(capturedSignal).toBeDefined());
    expect(capturedSignal?.aborted).toBe(false);
    view.unmount();
    expect(capturedSignal?.aborted).toBe(true);
  });

  it("keeps previous rows while switching outcome and loads user/key filters on demand", async () => {
    seedAuthenticatedSession();
    let usersRequests = 0;
    let apiKeysRequests = 0;
    let resolveFailed!: (value: Response) => void;
    const failedPending = new Promise<Response>((resolve) => {
      resolveFailed = resolve;
    });
    server.use(
      http.get("/console/v1/users", () => {
        usersRequests += 1;
        return HttpResponse.json({ items: [CONTROL_PLANE_USER], next_cursor: null, total: 1 });
      }),
      http.get("/console/v1/api-keys", () => {
        apiKeysRequests += 1;
        return HttpResponse.json({ items: [ADMIN_API_KEY], next_cursor: null, total: 1 });
      }),
      http.get("/console/v1/request-logs", () => HttpResponse.json({ items: [REQUEST_LOG], next_cursor: null })),
    );

    const user = userEvent.setup();
    await renderAppAt("/admin/request-logs");
    expect(await screen.findByText(REQUEST_LOG.client_model)).toBeVisible();
    expect(usersRequests).toBe(0);
    expect(apiKeysRequests).toBe(0);

    server.use(
      http.get("/console/v1/request-logs", () => failedPending),
    );
    await user.click(screen.getByRole("button", { name: "Failed" }));
    expect(screen.getByText(REQUEST_LOG.client_model)).toBeVisible();

    resolveFailed(HttpResponse.json({
      items: [{ ...REQUEST_LOG, id: "e1111111-2222-4333-8444-555555555555", client_model: "failed-model", outcome: "failed" }],
      next_cursor: null,
    }));
    expect(await screen.findByText("failed-model")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Filters (1)" }));
    expect(usersRequests).toBe(0);
    expect(apiKeysRequests).toBe(0);

    await user.click(await screen.findByRole("combobox", { name: "User" }));
    await waitFor(() => expect(usersRequests).toBe(1));
    await user.keyboard("{Escape}");
    await user.click(await screen.findByRole("combobox", { name: "API key" }));
    await waitFor(() => expect(apiKeysRequests).toBe(1));
  });

  it("paginates with cursor stack and resets cursor when filters change", async () => {
    seedAuthenticatedSession();
    const cursors: (string | null)[] = [];
    const page2Log = {
      ...REQUEST_LOG,
      id: "d1111111-2222-4333-8444-555555555555",
      client_model: "page-two-model",
    };
    server.use(
      http.get("/console/v1/request-logs", ({ request }) => {
        const cursor = new URL(request.url).searchParams.get("cursor");
        cursors.push(cursor);
        if (cursor === "cursor-p2") {
          return HttpResponse.json({ items: [page2Log], next_cursor: null });
        }
        return HttpResponse.json({ items: [REQUEST_LOG], next_cursor: "cursor-p2" });
      }),
    );

    const user = userEvent.setup();
    await renderAppAt("/admin/request-logs");
    expect(await screen.findByText(REQUEST_LOG.client_model)).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Go to next page" }));
    expect(await screen.findByText("page-two-model")).toBeVisible();
    expect(cursors.at(-1)).toBe("cursor-p2");

    await user.click(screen.getByRole("button", { name: "Failed" }));
    await waitFor(() => expect(cursors.at(-1)).toBeNull());
  });

  it("resolves URL user_id and api_key_id via single-record endpoints without listing", async () => {
    seedAuthenticatedSession();
    let userListCalls = 0;
    let keyListCalls = 0;
    server.use(
      http.get("/console/v1/users", () => {
        userListCalls += 1;
        return HttpResponse.json({ items: [], next_cursor: null, total: 0 });
      }),
      http.get("/console/v1/api-keys", () => {
        keyListCalls += 1;
        return HttpResponse.json({ items: [], next_cursor: null, total: 0 });
      }),
      http.get(`/console/v1/users/${CONTROL_PLANE_USER.id}`, () =>
        HttpResponse.json(
          { ...CONTROL_PLANE_USER, display_name: "Resolved User" },
          { headers: { ETag: '"user-etag"' } },
        ),
      ),
      http.get(`/console/v1/api-keys/${ADMIN_API_KEY.id}`, () =>
        HttpResponse.json(
          { ...ADMIN_API_KEY, name: "Resolved Key" },
          { headers: { ETag: '"key-etag"' } },
        ),
      ),
      http.get("/console/v1/request-logs", () =>
        HttpResponse.json({ items: [REQUEST_LOG], next_cursor: null }),
      ),
    );

    await renderAppAt(
      `/admin/request-logs?user_id=${CONTROL_PLANE_USER.id}&api_key_id=${ADMIN_API_KEY.id}`,
    );

    expect(await screen.findByText("User: Resolved User")).toBeVisible();
    expect(await screen.findByText("API key: Resolved Key · Resolved User")).toBeVisible();
    expect(userListCalls).toBe(0);
    expect(keyListCalls).toBe(0);
  });
});
