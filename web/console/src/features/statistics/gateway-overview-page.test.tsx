import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { STORAGE_KEY } from "@/app/i18n";
import { AppProviders } from "@/app/providers";
import { OWN_COST_STATISTICS_REPORT as report, REQUEST_LOG } from "@/test/fixtures";
import { seedAuthenticatedSession, seedUserSession, server } from "@/test/msw";
import { GatewayOverviewPage } from "./gateway-overview-page";
import { overviewWindow, usageSlices } from "./usage-window";

function renderOverview() {
  render(<AppProviders><MemoryRouter><GatewayOverviewPage /></MemoryRouter></AppProviders>);
}

function urlOf(link: HTMLElement) {
  return new URL(link.getAttribute("href")!, "https://console.test");
}

describe("Gateway overview", () => {
  it.each([["member", seedUserSession], ["administrator", seedAuthenticatedSession]] as const)(
    "queries only owner resources and drills into the failed request for a %s", async (_, seed) => {
      seed();
      const costs: URLSearchParams[] = [], summaries: URLSearchParams[] = [], logs: URLSearchParams[] = [], system: string[] = [];
      server.use(
        http.get("/console/v1/statistics/costs", ({ request }) => {
          costs.push(new URL(request.url).searchParams);
          return HttpResponse.json(report);
        }),
        http.get("/console/v1/statistics/costs/summary", ({ request }) => {
          summaries.push(new URL(request.url).searchParams);
          return HttpResponse.json(report.summary);
        }),
        http.get("/console/v1/me/request-logs", ({ request }) => {
          logs.push(new URL(request.url).searchParams);
          return HttpResponse.json([{ ...REQUEST_LOG, outcome: "failed", response_status_code: 503 }]);
        }),
        http.get(/\/console\/v1\/(system\/statistics\/costs|request-logs)$/, ({ request }) => {
          system.push(request.url);
          return new HttpResponse(null, {status: 403});
        }),
      );
      const user = userEvent.setup();
      renderOverview();
      const ledger = await screen.findByRole("tabpanel", {name: "Model distribution"});
      const model = urlOf(await within(ledger).findByRole("link", {name: `Related requests for ${report.models[0].model} · Chat Completions`}));
      expect(model.pathname).toBe("/usage/request-logs");
      expect(model.searchParams.get("api_format")).toBe(report.models[0].api_format);
      expect(model.searchParams.get("model")).toBe(report.models[0].model);
      expect(logs).toHaveLength(0);

      const interval = screen.getByRole("link", {name: "Interval logs"});
      expect(urlOf(interval).searchParams.get("started_after")).toBe("2026-07-21T00:00:00.000Z");
      expect(urlOf(interval).searchParams.get("started_before")).toBe("2026-07-21T13:59:59.999999Z");
      await user.click(screen.getByRole("button", {name: "Previous interval"}));
      expect(urlOf(interval).searchParams.get("started_after")).toBe("2026-07-20T00:00:00.000Z");
      const tokens = screen.getByRole("button", {name: /^Total tokens/});
      await user.click(tokens);
      expect(tokens).toHaveAttribute("aria-pressed", "true");
      expect(screen.getByText("16M Tokens")).toBeVisible();

      await user.click(screen.getByRole("button", {name: "Last 24 hours"}));
      const day = urlOf(screen.getByRole("link", {name: "Usage report"}));
      const start = day.searchParams.get("started_after")!, end = day.searchParams.get("started_before")!;
      expect(Date.parse(end)-Date.parse(start)).toBe(86_400_000);
      await waitFor(() => expect(costs.some(q => q.get("started_after") === start && q.get("started_before") === end && q.get("granularity") === "hour")).toBe(true));
      expect(costs).toHaveLength(2); // one full report per visited range
      const prior = overviewWindow(Date.parse(end), "24h", true);
      await waitFor(() => expect(Object.fromEntries(summaries.at(-1)!)).toEqual(prior));
      expect(costs.every(q => !q.has("user_id") && !q.has("channel_id"))).toBe(true);
      await user.click(screen.getByRole("tab", {name: "Failed requests"}));
      const failed = await screen.findByRole("tabpanel", {name: "Failed requests"});
      const request = urlOf(await within(failed).findByRole("link"));
      expect(request.pathname).toBe("/usage/request-logs");
      expect(request.searchParams.get("id")).toBe(REQUEST_LOG.id);
      expect(logs.at(-1)?.get("outcome")).toBe("failed");
      expect(logs.at(-1)?.get("started_after")).toBe(start);
      expect(logs.at(-1)?.get("started_before")).toBe(request.searchParams.get("started_before"));
      expect(system).toEqual([]);
    },
  );

  it("formats token and cost tooltips without replacing their series labels in Chinese", async () => {
    seedUserSession();
    const locale = localStorage.getItem(STORAGE_KEY);
    localStorage.setItem(STORAGE_KEY, "zh-CN");
    // jsdom has no layout; give the responsive chart a measurable viewport.
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return this.classList.contains("recharts-responsive-container") ? new DOMRect(0, 0, 600, 240) : new DOMRect();
    });
    try {
      const modelCost = "0.00000001";
      const bucket = report.buckets[0];
      server.use(http.get("/console/v1/statistics/costs", () => HttpResponse.json({
        ...report,
        summary: { ...report.summary, cost_amount: "9007199254740991.123456" },
        buckets: [{ ...bucket, cost_amount: modelCost, models: [{ ...bucket.models[0], cost_amount: modelCost }] }, report.buckets[1]],
      })));
      renderOverview();
      const user = userEvent.setup();
      const cost = await screen.findByRole("button", { name: /^总花费/ });
      expect(cost).toHaveTextContent("9,007,199,254,740,991.12346 USD");
      const chart = await screen.findByRole("application");
      const tooltip = within(chart.parentElement!.querySelector<HTMLElement>(".recharts-tooltip-wrapper")!);
      await user.click(screen.getByRole("button", { name: /^总 Token/ }));
      fireEvent.focus(chart);
      expect(await tooltip.findByText("16M")).toBeVisible();
      expect(tooltip.getByText(bucket.models[0].model)).toBeVisible();
      await user.click(cost);
      fireEvent.focus(chart);
      expect(await tooltip.findByText("<0.00001 USD")).toBeVisible();
      expect(tooltip.getByText(bucket.models[0].model)).toBeVisible();
    } finally {
      rect.mockRestore();
      if (locale === null) localStorage.removeItem(STORAGE_KEY);
      else localStorage.setItem(STORAGE_KEY, locale);
    }
  });

  it("keeps separate drill-downs for one requested model used in two formats", async () => {
    seedUserSession();
    const chat = report.models[0];
    server.use(http.get("/console/v1/statistics/costs", () => HttpResponse.json({
      ...report, models: [chat, {...chat, api_format: "open_ai_responses"}],
    })));
    renderOverview();
    const ledger = await screen.findByRole("tabpanel", {name: "Model distribution"});
    for (const [label, format] of [["Chat Completions", "open_ai_chat_completions"], ["Responses", "open_ai_responses"]]) {
      const link = await within(ledger).findByRole("link", {name: `Related requests for ${chat.model} · ${label}`});
      expect(Object.fromEntries(urlOf(link).searchParams)).toMatchObject({ model: chat.model, api_format: format });
    }
  });

  it("compares matching UTC windows across month boundaries and clips partial hourly slices", () => {
    const end = Date.parse("2026-03-02T00:15:00Z");
    expect(overviewWindow(end, "7d")).toEqual({ started_after:"2026-02-24T00:00:00.000Z", started_before:"2026-03-02T00:15:00.000Z" });
    expect(overviewWindow(end, "7d", true)).toEqual({ started_after:"2026-02-17T00:00:00.000Z", started_before:"2026-02-23T00:15:00.000Z" });
    const interval = usageSlices({ ...report, granularity: "hour", started_at:"2026-07-20T00:35:00Z", ended_at:"2026-07-20T01:12:00Z", buckets:[report.buckets[0], {...report.buckets[1],started_at:"2026-07-20T01:00:00Z"}] }, false);
    expect(interval.map(s => [s.start,s.end])).toEqual([
      [Date.parse("2026-07-20T00:35:00Z"),Date.parse("2026-07-20T01:00:00Z")],
      [Date.parse("2026-07-20T01:00:00Z"),Date.parse("2026-07-20T01:12:00Z")],
    ]);
  });
});
