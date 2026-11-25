import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedUserSession, server } from "@/test/msw";
import { SPEND_LEADERBOARD_REPORT } from "@/test/fixtures";

function renderApp() {
  window.history.replaceState({}, "", "/leaderboard");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("SpendLeaderboardPage", () => {
  it("lets regular users browse fixed natural-period leaderboard snapshots", async () => {
    seedUserSession();
    const queries: URLSearchParams[] = [];
    server.use(
      http.get("/console/v1/statistics/spend-leaderboard", ({ request }) => {
        const query = new URL(request.url).searchParams;
        queries.push(query);
        const period = query.get("period") ?? "day";
        const defaultStart = period === "week" ? "2026-07-20" : "2026-07-21";
        return HttpResponse.json({
          ...SPEND_LEADERBOARD_REPORT,
          period,
          period_start: query.get("period_start") ?? defaultStart,
          period_end: period === "week" ? "2026-07-27" : "2026-07-22",
          previous_period_start: period === "week" ? "2026-07-13" : "2026-07-20",
          next_period_start: null,
        });
      }),
    );
    const user = userEvent.setup();
    renderApp();

    expect(
      await screen.findByRole("heading", { name: "Spend leaderboard" }),
    ).toBeInTheDocument();
    const list = await screen.findByRole("list", { name: "User rankings" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(4);
    expect(rows[0]).toHaveTextContent("Rank 1");
    expect(rows[0]).toHaveTextContent("Ada Lovelace");
    expect(rows[0]).toHaveTextContent("721.95 USD");
    expect(rows[0]).toHaveTextContent("37.8%");
    expect(rows[0]).toHaveTextContent("98.3M");
    expect(rows[1]).toHaveTextContent("Diego Rivera");
    expect(screen.getByText("1,912.06 USD")).toBeInTheDocument();
    expect(screen.getByText("Shanghai time · Asia/Shanghai")).toBeInTheDocument();
    await waitFor(() => {
      expect(queries.at(-1)?.get("period")).toBe("day");
      expect(queries.at(-1)?.has("period_start")).toBe(false);
      expect(queries.at(-1)?.get("limit")).toBe("50");
    });

    await user.click(screen.getByRole("button", { name: "Weekly ranking" }));
    await waitFor(() => {
      expect(queries.at(-1)?.get("period")).toBe("week");
      expect(queries.at(-1)?.has("period_start")).toBe(false);
    });

    await user.click(screen.getByRole("button", { name: "Previous period" }));
    await waitFor(() => {
      expect(queries.at(-1)?.get("period_start")).toBe("2026-07-13");
    });
    await user.click(screen.getByRole("button", { name: "Back to current period" }));
    expect(await screen.findByRole("button", { name: "Current ranking period" })).toHaveAttribute("aria-disabled", "true");
    expect(await screen.findByText("Jul 20, 2026 – Jul 26, 2026")).toBeInTheDocument();
  });
});
