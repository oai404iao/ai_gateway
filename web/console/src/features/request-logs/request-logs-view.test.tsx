import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  ADMIN_API_KEY,
  CONTROL_PLANE_USER,
  MODEL_RULE,
  OWN_API_KEY,
  REQUEST_LOG,
} from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";

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
  it("sends the filter bar values as server-side request-log query parameters", async () => {
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

    const apiKey = await screen.findByRole("combobox", { name: "API key" });
    await user.click(apiKey);
    await user.click(
      await screen.findByRole("option", {
        name: `${OWN_API_KEY.name} · ${OWN_API_KEY.id.slice(0, 8)}`,
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

  it("offers administrator user, API key, and configured model dropdowns", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    server.use(
      http.get("/console/v1/request-logs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json([]);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/admin/request-logs");

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
        name: `${ADMIN_API_KEY.name} · ${ADMIN_API_KEY.id.slice(0, 8)}`,
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
      api_format: "open_ai_responses",
      client_model: "detail-model",
      upstream_model: "upstream-detail-model",
      model_rule_id: null,
      channel_group_id: null,
      channel_id: null,
      outcome: "succeeded",
      response_status_code: 200,
      streamed: true,
      ttft_ms: 100,
      total_duration_ms: 1_000,
      input_tokens: 12,
      cached_input_tokens: 2,
      cache_write_tokens: 0,
      output_tokens: 4,
      currency: "USD",
      cost_amount: "0.0001",
      error_code: null,
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
    expect(
      await screen.findByText("upstream-detail-model", { selector: "dd" }),
    ).toBeInTheDocument();
    expect(detailRequests).toBe(1);
  });
});
