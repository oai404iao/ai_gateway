import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
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
  MODEL_RULE,
  OWN_API_KEY,
  REQUEST_LOG,
} from "@/test/fixtures";
import { server, seedAuthenticatedSession, seedUserSession } from "@/test/msw";

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("RequestLogsView", () => {
  it("refetches when the same request-log filters are applied again", async () => {
    seedAuthenticatedSession();
    let requests = 0;
    server.use(
      http.get("/console/v1/me/request-logs", () => {
        requests += 1;
        return HttpResponse.json([REQUEST_LOG]);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/usage/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    await waitFor(() => expect(requests).toBe(1));

    const apply = screen.getByRole("button", { name: "Apply" });
    await user.click(apply);
    await waitFor(() => expect(requests).toBe(2));
    await waitFor(() => expect(apply).toBeEnabled());

    await user.click(apply);
    await waitFor(() => expect(requests).toBe(3));
  });

  it("keeps the personal request-log table owner-scoped even for administrators", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    server.use(
      http.get("/console/v1/me/request-logs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json([REQUEST_LOG]);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/usage/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    expect(
      screen.getAllByRole("columnheader").map((header) => header.textContent),
    ).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
    ]);
    expect(screen.getByText("SSE")).toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.name)).not.toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.id)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Uncached input: 10")).toHaveTextContent("10");
    expect(screen.getByLabelText("Cached input: 2")).toHaveTextContent("2");
    expect(screen.getByLabelText("Non-reasoning output: 3")).toHaveTextContent("3");
    expect(screen.getByLabelText("Reasoning tokens: 1")).toHaveTextContent("1");
    expect(screen.getByLabelText("Reasoning effort: High")).toHaveTextContent("High");
    expect(screen.getByLabelText("Fast mode")).toHaveTextContent("Fast");
    const ttft = screen.getByLabelText("TTFT: 100 ms");
    const totalDuration = screen.getByLabelText("Total duration: 1 s");
    const tps = screen.getByLabelText("TPS: 4.4 tok/s");
    expect(ttft.parentElement).toContainElement(totalDuration);
    expect(ttft.parentElement).toContainElement(tps);
    expect(ttft).toHaveClass("text-muted-foreground");
    expect(tps).toHaveClass("text-muted-foreground");

    const apiKey = await screen.findByRole("combobox", { name: "API key" });
    await user.click(apiKey);
    await user.click(
      await screen.findByRole("option", {
        name: OWN_API_KEY.name,
      }),
    );

    const model = screen.getByRole("combobox", { name: "Model" });
    await user.click(model);
    await user.click(
      await screen.findByRole("option", { name: REQUEST_LOG.client_model }),
    );
    await user.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(
        queries.some(
          (query) =>
            query.get("api_key_id") === OWN_API_KEY.id &&
            query.get("model") === REQUEST_LOG.client_model,
        ),
      ).toBe(true);
    });
  });

  it("hides request-mode badges when the client did not enable them", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/me/request-logs", () =>
        HttpResponse.json([
          {
            ...REQUEST_LOG,
            reasoning_effort: null,
            fast_mode: false,
          },
        ]),
      ),
    );

    renderAppAt("/usage/request-logs");

    await screen.findByText(REQUEST_LOG.client_model);
    expect(screen.queryByLabelText(/Reasoning effort:/)).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Fast mode")).not.toBeInTheDocument();
  });

  it("adds user and channel names only to the system request-log table", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    server.use(
      http.get("/console/v1/request-logs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json([REQUEST_LOG]);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/admin/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    expect(
      screen.getAllByRole("columnheader").map((header) => header.textContent),
    ).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Channel",
      "User",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
    ]);
    expect(screen.getByText(CHANNEL_GROUP.name)).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: CHANNEL.name })).toBeInTheDocument();
    expect(
      screen.getByRole("cell", { name: CONTROL_PLANE_USER.display_name }),
    ).toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.id)).not.toBeInTheDocument();
    expect(screen.queryByText(CONTROL_PLANE_USER.id)).not.toBeInTheDocument();

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
        name: `${ADMIN_API_KEY.name} · ${CONTROL_PLANE_USER.display_name}`,
      }),
    );

    const modelSelect = screen.getByRole("combobox", { name: "Model" });
    await user.click(modelSelect);
    await user.click(
      await screen.findByRole("option", { name: MODEL_RULE.client_model }),
    );
    await user.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(
        queries.some(
          (query) =>
            query.get("user_id") === CONTROL_PLANE_USER.id &&
            query.get("api_key_id") === ADMIN_API_KEY.id &&
            query.get("model") === MODEL_RULE.client_model,
        ),
      ).toBe(true);
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
      streamed: true,
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
      http.get("/console/v1/request-logs", () => HttpResponse.json([log])),
      http.get("/console/v1/request-logs/:id", ({ params }) => {
        expect(params.id).toBe(log.id);
        detailRequests += 1;
        return HttpResponse.json(log);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/admin/request-logs");

    await user.click(await screen.findByText("detail-model"));
    await screen.findByText("Completed", { selector: "dt" });
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
      "Error message",
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
    const errorMessageLabel = await screen.findByText("Error message", { selector: "dt" });
    expect(errorMessageLabel.parentElement).toHaveTextContent(
      "Upstream quota exhausted. Try another channel.",
    );
    expect(screen.queryByText(CHANNEL.id)).not.toBeInTheDocument();
    expect(screen.queryByText(log.user_id)).not.toBeInTheDocument();
    expect(detailRequests).toBe(1);
  });

  it("limits personal details to the standardized owner-visible fields", async () => {
    seedUserSession();
    const ownLog = {
      ...REQUEST_LOG,
      user_name: null,
      channel_id: null,
      channel_name: null,
    };
    server.use(
      http.get("/console/v1/me/request-logs", () =>
        HttpResponse.json([ownLog]),
      ),
      http.get("/console/v1/me/request-logs/:id", () =>
        HttpResponse.json(ownLog),
      ),
    );

    const user = userEvent.setup();
    renderAppAt("/usage/request-logs");

    await screen.findByRole("columnheader", { name: "Started" });
    expect(
      screen.getAllByRole("columnheader").map((header) => header.textContent),
    ).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
    ]);
    expect(await screen.findByText(CHANNEL_GROUP.name)).toBeInTheDocument();

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
      "Error message",
      "Completed",
    ]);
    const groupLabel = await screen.findByText("Channel group", { selector: "dt" });
    expect(groupLabel.parentElement).toHaveTextContent(CHANNEL_GROUP.name);
    expect(screen.queryByText("Channel", { selector: "dt" })).not.toBeInTheDocument();
    expect(screen.queryByText("User", { selector: "dt" })).not.toBeInTheDocument();
    expect(screen.queryByText(CHANNEL.name, { selector: "dd" })).not.toBeInTheDocument();
  });
});
