import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession, seedUserSession, server } from "@/test/msw";
import {
  CHANNEL,
  CHANNEL_GROUP,
  COST_STATISTICS_REPORT,
  MODEL,
} from "@/test/fixtures";

function renderApp(path = "/statistics") {
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("StatisticsPage", () => {
  it("redirects the Console root to statistics", async () => {
    seedAuthenticatedSession();
    renderApp("/");

    expect(await screen.findByText("Total cost")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/statistics");
  });

  it("shows cost and system load analytics", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderApp();

    expect(await screen.findByText("Total cost", {}, { timeout: 5_000 })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Price sync" })).toBeInTheDocument();
    expect((await screen.findAllByText(MODEL.source_model_id)).length).toBeGreaterThan(0);
    expect((await screen.findAllByText("97.5%")).length).toBeGreaterThan(0);

    expect(screen.getAllByText("1,912.06 USD").length).toBeGreaterThan(0);
    expect(screen.getByText("Model cost breakdown")).toBeInTheDocument();
    expect(screen.getByText("Channel details")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Channel" })).toBeInTheDocument();
    expect(screen.getAllByText(CHANNEL.name).length).toBeGreaterThan(0);
    expect(screen.getByText(/Cache rate: 40%/)).toBeInTheDocument();
    expect(
      screen.getAllByRole("columnheader", { name: "Input tokens" }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("columnheader", { name: "Cache hit tokens" }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("columnheader", { name: "Cache rate" }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("columnheader", { name: "Cache write tokens" }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("columnheader", { name: "Output tokens" }).length,
    ).toBeGreaterThan(0);
    expect(screen.getAllByText("210M").length).toBeGreaterThan(0);
    expect(screen.getAllByText("84M").length).toBeGreaterThan(0);
    expect(screen.getAllByText("12M").length).toBeGreaterThan(0);
    expect(screen.getAllByText("53M").length).toBeGreaterThan(0);
    expect(screen.getAllByText("40%").length).toBeGreaterThan(0);

    await user.click(screen.getByRole("tab", { name: "System load" }));

    expect(await screen.findByText("Host CPU")).toBeInTheDocument();
    expect(screen.getByText("42.5%")).toBeInTheDocument();
    expect(screen.getByText("Bounded queues")).toBeInTheDocument();
    expect(screen.getByText("Request-log notifications")).toBeInTheDocument();
    expect(screen.getByText("2 MiB")).toBeInTheDocument();
  });

  it("keeps regular-user statistics scoped to their own costs", async () => {
    seedUserSession();
    let adminUserRequests = 0;
    let adminKeyRequests = 0;
    server.use(
      http.get("/console/v1/users", () => {
        adminUserRequests += 1;
        return new HttpResponse(null, { status: 403 });
      }),
      http.get("/console/v1/api-keys", () => {
        adminKeyRequests += 1;
        return new HttpResponse(null, { status: 403 });
      }),
    );
    renderApp();

    expect(await screen.findByText("Total cost", {}, { timeout: 5_000 })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Statistics" })).toHaveAttribute(
      "href",
      "/statistics",
    );
    expect(screen.getByRole("link", { name: "Channel status" })).toHaveAttribute(
      "href",
      "/channel-status",
    );
    expect(screen.queryByRole("link", { name: "Price sync" })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "System load" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Channel" })).not.toBeInTheDocument();
    expect(screen.queryByText("Channel details")).not.toBeInTheDocument();

    expect(
      screen.getByText(
        "Filter your own statistics by time range, API key, and aggregation granularity.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("All users")).not.toBeInTheDocument();
    expect(adminUserRequests).toBe(0);
    expect(adminKeyRequests).toBe(0);
  });

  it("shows public channel status on its own page", async () => {
    seedUserSession();
    renderApp("/channel-status");

    expect(await screen.findByRole("heading", { name: "Channel status" })).toBeInTheDocument();
    expect(await screen.findByText("Model overview")).toBeInTheDocument();
    expect(await screen.findByText(CHANNEL.name)).toBeInTheDocument();
    expect(screen.queryByText("Total cost")).not.toBeInTheDocument();
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
    expect(screen.getByRole("button", { name: "Today" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    const channelSelect = screen.getByRole("combobox", { name: "Channel" });
    await user.click(channelSelect);
    await user.click(
      await screen.findByRole("option", {
        name: `${CHANNEL_GROUP.name} · ${CHANNEL.name}`,
      }),
    );
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => {
      expect(queries.at(-1)?.get("channel_id")).toBe(CHANNEL.id);
    });

    const todayStart = new Date(queries.at(-1)?.get("started_after") ?? "");
    const todayEnd = new Date(queries.at(-1)?.get("started_before") ?? "");
    expect(todayStart.getHours()).toBe(0);
    expect(todayStart.getMinutes()).toBe(0);
    expect(todayStart.toDateString()).toBe(todayEnd.toDateString());

    await user.click(screen.getByRole("button", { name: "This week" }));
    await waitFor(() => {
      expect(queries.at(-1)?.get("granularity")).toBe("day");
      const startedAt = new Date(queries.at(-1)?.get("started_after") ?? "");
      expect(startedAt.getDay()).toBe(1);
      expect(startedAt.getHours()).toBe(0);
    });

    await user.click(screen.getByRole("button", { name: "This month" }));
    await waitFor(() => {
      const startedAt = new Date(queries.at(-1)?.get("started_after") ?? "");
      expect(startedAt.getDate()).toBe(1);
      expect(startedAt.getHours()).toBe(0);
    });
  });
});
