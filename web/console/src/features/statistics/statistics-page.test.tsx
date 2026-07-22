import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession, server } from "@/test/msw";
import {
  CHANNEL,
  COST_STATISTICS_REPORT,
  MODEL,
} from "@/test/fixtures";

function renderApp() {
  window.history.replaceState({}, "", "/admin/statistics");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("StatisticsPage", () => {
  it("shows channel, cost, and system load analytics", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp();

    expect(
      await screen.findByText("Model overview", {}, { timeout: 5_000 }),
    ).toBeInTheDocument();
    expect((await screen.findAllByText(MODEL.source_model_id)).length).toBeGreaterThan(0);
    expect(await screen.findByText(CHANNEL.name)).toBeInTheDocument();
    expect((await screen.findAllByText("97.5%")).length).toBeGreaterThan(0);

    await user.click(screen.getByRole("tab", { name: "Cost statistics" }));

    expect(await screen.findByText("Total cost")).toBeInTheDocument();
    expect(screen.getAllByText("1,912.06 USD").length).toBeGreaterThan(0);
    expect(screen.getByText("Model cost breakdown")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "System load" }));

    expect(await screen.findByText("Host CPU")).toBeInTheDocument();
    expect(screen.getByText("42.5%")).toBeInTheDocument();
    expect(screen.getByText("Bounded queues")).toBeInTheDocument();
    expect(screen.getByText("Request-log notifications")).toBeInTheDocument();
    expect(screen.getByText("2 MiB")).toBeInTheDocument();
  });

  it("defaults to today and applies week and month ranges with useful granularities", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    server.use(
      http.get("/console/v1/statistics/costs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json(COST_STATISTICS_REPORT);
      }),
    );
    const user = userEvent.setup();
    renderApp();

    await user.click(await screen.findByRole("tab", { name: "Cost statistics" }));
    await waitFor(() => {
      expect(queries.length).toBeGreaterThan(0);
      expect(queries.at(-1)?.get("granularity")).toBe("hour");
    });
    expect(screen.getByRole("radio", { name: "Today" })).toBeChecked();
    const todayStart = new Date(queries.at(-1)?.get("started_after") ?? "");
    const todayEnd = new Date(queries.at(-1)?.get("started_before") ?? "");
    expect(todayStart.getHours()).toBe(0);
    expect(todayStart.getMinutes()).toBe(0);
    expect(todayStart.toDateString()).toBe(todayEnd.toDateString());

    await user.click(screen.getByRole("radio", { name: "This week" }));
    await waitFor(() => {
      expect(queries.at(-1)?.get("granularity")).toBe("day");
      const startedAt = new Date(queries.at(-1)?.get("started_after") ?? "");
      expect(startedAt.getDay()).toBe(1);
      expect(startedAt.getHours()).toBe(0);
    });

    await user.click(screen.getByRole("radio", { name: "This month" }));
    await waitFor(() => {
      const startedAt = new Date(queries.at(-1)?.get("started_after") ?? "");
      expect(startedAt.getDate()).toBe(1);
      expect(startedAt.getHours()).toBe(0);
    });
  });
});
