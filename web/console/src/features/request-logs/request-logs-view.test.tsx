import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type { RequestLogView } from "@/api/types";
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
        return HttpResponse.json([]);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/usage/request-logs");

    const model = await screen.findByLabelText("Model");
    await user.type(model, "gpt-4o-mini");
    await user.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(queries.some((query) => query.get("model") === "gpt-4o-mini")).toBe(true);
    });
  });

  it("loads administrator request-log details from the admin detail endpoint", async () => {
    seedAuthenticatedSession();
    const log: RequestLogView = {
      id: "11111111-2222-4333-8444-555555555555",
      started_at: "2026-07-21T06:00:00Z",
      completed_at: "2026-07-21T06:00:01Z",
      user_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      api_key_id: "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
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
    };
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
    expect(await screen.findByText("upstream-detail-model")).toBeInTheDocument();
    expect(detailRequests).toBe(1);
  });
});
