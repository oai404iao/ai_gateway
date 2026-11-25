import { expect, test, type Locator, type Page, type Route, type TestInfo } from "@playwright/test";
import type { SpendLeaderboardEntry, SpendLeaderboardReport } from "../src/api/types";
import { E2E_USER, mockConsoleApi } from "./mock-api";

const api = "**/console/v1/statistics/spend-leaderboard?**";
const names = ["星桥研发", "Oak Lab", "Lin Qiao", "灰鲸工作室", "Northwind QA", "安禾", "杉林数据"];
const costs = ["721.95", "602.12", "385.12", "89.70", "68.00", "31.60", "13.57"];
const pricedRequests = [6800, 5000, 3085, 819, 724, 336, 139];
const tokens = [98300000, 74200000, 51400000, 12100000, 6900000, 3860000, 1750000];

function entry(rank: number, display_name: string, cost_amount: string): SpendLeaderboardEntry {
  return {
    rank, display_name, cost_amount,
    user_id: `10000000-0000-0000-0000-${String(rank).padStart(12, "0")}`,
    priced_request_count: pricedRequests[rank - 1] ?? 1,
    request_count: (pricedRequests[rank - 1] ?? 1) + 8,
    total_tokens: tokens[rank - 1] ?? 1000,
  };
}

const auditReport: SpendLeaderboardReport = {
  period: "day",
  period_start: "2026-07-24",
  period_end: "2026-07-25",
  refreshed_at: "2026-07-24T12:15:00Z",
  previous_period_start: "2026-07-23",
  next_period_start: null,
  total_cost_amount: "1912.06",
  entries: names.map((name, i) => entry(i + 1, name, costs[i])),
};

type SnapshotResponse = SpendLeaderboardReport | { error: string };
type SnapshotHandler = (period: string, start: string | null) => SnapshotResponse | Promise<SnapshotResponse>;

async function signIn(page: Page, handle: SnapshotHandler, settings: { locale?: string; theme?: string } = {}) {
  await page.addInitScript(({ locale, theme }) => {
    localStorage.setItem("ai-gateway-console.locale", locale ?? "en-US");
    localStorage.setItem("console-theme", theme ?? "light");
  }, settings);
  const queries: URLSearchParams[] = [];
  await mockConsoleApi(page);
  await page.route(api, async (route: Route) => {
    expect(route.request().method()).toBe("GET");
    expect(route.request().headers().authorization).toBe("Bearer e2e-user-token");
    const { searchParams } = new URL(route.request().url());
    expect(searchParams.getAll("limit")).toEqual(["50"]);
    queries.push(searchParams);
    const result = await handle(searchParams.get("period") ?? "", searchParams.get("period_start"));
    await route.fulfill({ status: "error" in result ? 502 : 200, json: result });
  });

  await page.goto("/login");
  await page.locator("input[type=email]").fill(E2E_USER.email);
  await page.locator("input[type=password]").fill("mock-password");
  await page.locator("button[type=submit]").click();
  await expect(page).toHaveURL("/");
  await page.goto("/leaderboard");
  return queries;
}

function rankingPeriod(page: Page) {
  return page.getByRole("group", { name: "Ranking period" });
}

function tableRows(page: Page) {
  return page.getByRole("table").getByRole("row").filter({ has: page.getByRole("cell") });
}

function rankedList(page: Page, label = "User rankings") {
  return page.getByRole("list", { name: label });
}

function lastQuery(queries: URLSearchParams[]) {
  return Object.fromEntries(queries.at(-1) ?? []);
}

async function press(page: Page, control: Locator, key = "Enter") {
  await expect(control).toBeEnabled();
  await control.focus();
  await expect(control).toBeFocused();
  await page.keyboard.press(key);
}

async function firstViewport(page: Page, locator: Locator) {
  await expect(locator).toBeInViewport({ ratio: 1 });
  const bounds = await locator.boundingBox();
  const toolbar = await page.getByRole("banner").boundingBox();
  expect(bounds!.y).toBeGreaterThanOrEqual(toolbar!.y + toolbar!.height);
  const dock = page.getByRole("navigation", { name: /Mobile navigation|移动导航/ });
  if (await dock.isVisible()) {
    const nav = (await dock.boundingBox())!;
    if (await page.evaluate(() => matchMedia("(max-width: 24rem)").matches)) {
      expect(bounds!.y).toBeGreaterThanOrEqual(nav.y + nav.height);
    } else {
      expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(nav.y);
    }
  }
}

async function screenshot(page: Page, name: string, info: TestInfo) {
  await page.evaluate(() => document.fonts.ready);
  await info.attach(name, { body: await page.screenshot(), contentType: "image/png" });
}

test.use({ serviceWorkers: "block", timezoneId: "America/Los_Angeles", reducedMotion: "reduce" });

for (const [layout, width, height] of [["wide", 1440, 900], ["narrow", 390, 844]] as const) {
  test(`${layout} shows the date, total and the first three unique rankings in the first viewport`, async ({ page }, info) => {
    await page.setViewportSize({ width, height });
    await page.clock.setFixedTime(new Date("2026-07-24T12:17:00Z"));
    const queries = await signIn(page, (period, start) => {
      expect({ period, start }).toEqual({ period: "day", start: null });
      return auditReport;
    }, { locale: "zh-CN" });
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("花费排行榜");
    const rankings = layout === "wide" ? tableRows(page) : rankedList(page, "用户排名").getByRole("listitem");
    await expect(rankings).toHaveCount(7);
    for (const [index, name] of names.entries()) {
      await expect(rankings.nth(index).getByText(name, { exact: true })).toBeVisible();
      await expect(page.locator("main").getByText(name, { exact: true }).filter({ visible: true })).toHaveCount(1);
      if (index < 3) {
        await firstViewport(page, rankings.nth(index).getByText(name, { exact: true }));
        await firstViewport(page, rankings.nth(index).getByText(`${costs[index]} USD`, { exact: true }));
      }
    }
    await expect(layout === "wide" ? rankedList(page, "用户排名") : page.getByRole("table")).toHaveCount(0);
    await firstViewport(page, page.getByRole("status").filter({ hasText: /^2026年\s*7月\s*24日$/ }));
    await firstViewport(page, page.getByText("1,912.06 USD", { exact: true }));
    await expect(rankings.first().getByText(/37\.8%/)).toBeVisible();
    await expect(rankings.first().getByText("98.3M", { exact: true })).toBeVisible();
    expect(await page.evaluate(() => scrollY)).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
    expect(lastQuery(queries)).toEqual({ period: "day", limit: "50" });
    await info.attach(`geometry-${layout}`, {
      body: JSON.stringify({ width, height, firstRanking: await rankings.first().boundingBox() }),
      contentType: "application/json",
    });
    await screenshot(page, `after-${layout}`, info);
  });
}

test("keyboard period controls use Shanghai dates and the server's retained history", async ({ page }, info) => {
  await page.clock.setFixedTime(new Date("2026-07-30T16:17:00Z"));
  const reports: Record<string, SpendLeaderboardReport> = {
    "day/": { ...auditReport, period_start: "2026-07-31", period_end: "2026-08-01", refreshed_at: "2026-07-30T16:15:00Z",
      previous_period_start: "2026-07-24" },
    "day/2026-07-24": { ...auditReport, total_cost_amount: "35", entries: [entry(1, "Older daily row", "35")],
      previous_period_start: "2026-07-17", next_period_start: "2026-07-31" },
    "day/2026-07-17": { ...auditReport, period_start: "2026-07-17", period_end: "2026-07-18", refreshed_at: "2026-07-17T12:15:00Z",
      total_cost_amount: "10", entries: [entry(1, "Earliest daily row", "10")], previous_period_start: null, next_period_start: "2026-07-24" },
    "week/": { ...auditReport, period: "week", period_start: "2026-07-27", period_end: "2026-08-03", refreshed_at: "2026-07-30T16:00:00Z",
      total_cost_amount: "81.00", entries: [entry(1, "Weekly row", "81")], previous_period_start: null },
    "month/": { ...auditReport, period: "month", period_start: "2026-07-01", period_end: "2026-08-01", refreshed_at: "2026-07-30T16:15:00Z",
      total_cost_amount: "202.00", entries: [entry(1, "Monthly row", "202")], previous_period_start: null },
  };
  const queries = await signIn(page, (period, start) => {
    const report = reports[`${period}/${start ?? ""}`];
    expect(report, `unexpected snapshot ${period}/${start ?? ""}`).toBeDefined();
    return report;
  });
  await expect(page.getByRole("status").filter({ hasText: /^Jul\.? 31,? 2026$/ })).toBeVisible();
  await expect(page.locator("time")).toContainText(/Jul\.? 31,? 2026.*00:15/);
  const periods = rankingPeriod(page);
  const daily = periods.getByRole("button", { name: "Daily ranking" });
  const weekly = periods.getByRole("button", { name: "Weekly ranking" });
  const monthly = periods.getByRole("button", { name: "Monthly ranking" });
  const previous = page.getByRole("button", { name: "Previous period" });
  const next = page.getByRole("button", { name: "Next period" });
  const current = page.getByRole("button", { name: "Back to current period" });
  await expect(daily).toHaveAttribute("aria-pressed", "true");
  await expect(next).toBeDisabled();

  await press(page, previous);
  await expect(tableRows(page).getByRole("cell", { name: "Older daily row", exact: true })).toBeVisible();
  expect(lastQuery(queries)).toEqual({ period: "day", period_start: "2026-07-24", limit: "50" });
  await press(page, previous);
  await expect(tableRows(page).getByRole("cell", { name: "Earliest daily row", exact: true })).toBeVisible();
  expect(lastQuery(queries)).toEqual({ period: "day", period_start: "2026-07-17", limit: "50" });
  await expect(previous).toBeDisabled();
  await expect(previous).toBeFocused();
  await screenshot(page, "after-retained-history", info);
  await press(page, next);
  await expect(tableRows(page).getByRole("cell", { name: "Older daily row", exact: true })).toBeVisible();

  await daily.focus();
  await page.keyboard.press("ArrowRight");
  await expect(weekly).toBeFocused();
  await page.keyboard.press("Space");
  await expect(tableRows(page).getByRole("cell", { name: "Weekly row", exact: true })).toBeVisible();
  await expect(weekly).toBeFocused();
  const crossMonthRange = page.getByRole("status").filter({ hasText: /Jul\.? 27,? 2026\s*[–—-]\s*Aug\.? 2,? 2026/ });
  await expect(crossMonthRange).toBeVisible();
  await expect(page.getByText(/Aug\.? 3,? 2026/)).toHaveCount(0);
  expect(lastQuery(queries)).toEqual({ period: "week", limit: "50" });

  const desktop = page.viewportSize()!;
  await page.setViewportSize({ width: 320, height: 720 });
  await firstViewport(page, periods);
  await firstViewport(page, crossMonthRange);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(320);
  await screenshot(page, "after-320-cross-month", info);
  await page.setViewportSize(desktop);

  await page.keyboard.press("ArrowRight");
  await expect(monthly).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(tableRows(page).getByRole("cell", { name: "Monthly row", exact: true })).toBeVisible();
  await expect(page.getByText("July 2026", { exact: true })).toBeVisible();
  expect(lastQuery(queries)).toEqual({ period: "month", limit: "50" });
  await press(page, daily);
  await expect(tableRows(page).getByRole("cell", { name: "星桥研发", exact: true })).toBeVisible();
  await press(page, previous);
  await expect(tableRows(page).getByRole("cell", { name: "Older daily row", exact: true })).toBeVisible();
  await press(page, current);
  await expect(tableRows(page).getByRole("cell", { name: "星桥研发", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Current ranking period" })).toBeFocused();
  await expect(next).toBeDisabled();
  expect(queries.filter((query) => query.get("period_start") === null).map((query) => query.get("period")))
    .toEqual(["day", "week", "month"]);
});

test("uncached and out-of-order responses keep date, refresh time, amounts and rows in the same snapshot", async ({ page }) => {
  let release!: () => void;
  const pause = new Promise<void>((resolve) => { release = resolve; });
  const queries = await signIn(page, async (period, start) => {
    expect(start).toBeNull();
    if (period === "day") return auditReport;
    if (period === "week") {
      await pause;
      return { ...auditReport, period, period_start: "2026-07-20", period_end: "2026-07-27", refreshed_at: "2026-07-24T10:15:00Z",
        total_cost_amount: "5.25", entries: [entry(1, "Late weekly row", "5.25")] };
    }
    expect(period).toBe("month");
    return { ...auditReport, period: "month", period_start: "2026-07-01", period_end: "2026-08-01", refreshed_at: "2026-07-24T14:45:00Z",
      total_cost_amount: "902.95", entries: [entry(1, "Recent monthly row", "810")] };
  });
  await expect(page.getByText("1,912.06 USD", { exact: true })).toBeVisible();
  await press(page, rankingPeriod(page).getByRole("button", { name: "Weekly ranking" }));
  await expect.poll(() => lastQuery(queries)).toEqual({ period: "week", limit: "50" });
  await expect(page.getByText("1,912.06 USD", { exact: true })).toHaveCount(0);
  await expect(page.getByText("星桥研发", { exact: true })).toHaveCount(0);
  await expect(page.getByText(/20:15/)).toHaveCount(0);
  await expect(page.getByText(/^0(?:\.00)? USD$/)).toHaveCount(0);

  await press(page, rankingPeriod(page).getByRole("button", { name: "Monthly ranking" }));
  await expect(tableRows(page).getByRole("cell", { name: "Recent monthly row", exact: true })).toBeVisible();
  const late = page.waitForResponse((response) => new URL(response.url()).searchParams.get("period") === "week");
  release();
  await late;
  await expect(page.getByText("902.95 USD", { exact: true })).toBeVisible();
  await expect(page.getByText(/22:45/)).toBeVisible();
  await expect(page.getByText("July 2026", { exact: true })).toBeVisible();
  await expect(tableRows(page).getByRole("cell", { name: "Recent monthly row", exact: true })).toBeVisible();
  await expect(page.getByText("Late weekly row", { exact: true })).toHaveCount(0);
  await expect(page.getByText(/18:15/)).toHaveCount(0);
  expect(lastQuery(queries)).toEqual({ period: "month", limit: "50" });
});

test("pagination uses server rank and the whole period denominator, not the visible members", async ({ page }) => {
  const entries = [
    entry(1, "Largest", "200"),
    entry(2, "Same display name", "50"),
    entry(3, "Same display name", "50"),
    ...Array.from({ length: 47 }, (_, index) => entry(index + 4, `Participant ${index + 4}`, index < 44 ? "15" : "10")),
  ];
  const weeklyEntries = [entry(1, "Largest", "90.05"), entry(2, "Halfway rounding", "9.95"),
    ...entries.slice(2).map((item) => ({ ...item, cost_amount: "0" }))];
  const queries = await signIn(page, (period, start) => {
    expect(start).toBeNull();
    return period === "week" ? { ...auditReport, period, period_start: "2026-07-20", period_end: "2026-07-27",
      total_cost_amount: "100", entries: weeklyEntries } : { ...auditReport, total_cost_amount: "1000", entries };
  });
  const rows = tableRows(page);
  await expect(rows).toHaveCount(20);
  await expect(rows.first().getByRole("cell", { name: "20%", exact: true })).toBeVisible();
  await expect(rows.first().getByRole("cell", { name: "20.2%", exact: true })).toHaveCount(0);
  await expect(rows.first().getByRole("cell", { name: "6,800", exact: true })).toBeVisible();
  await expect(rows.first().getByRole("cell", { name: "6,808", exact: true })).toHaveCount(0);
  await expect(rows.getByRole("cell", { name: "Same display name", exact: true })).toHaveCount(2);
  await expect(rows.first().getByRole("cell", { name: /^(?:Rank )?#?1$/ })).toBeVisible();
  await expect(page.locator("main")).not.toContainText(entries[1].user_id);

  await press(page, page.getByRole("button", { name: "Go to next page" }));
  await expect(rows.first()).toContainText("Participant 21");
  await expect(rows.first().getByRole("cell", { name: /^(?:Rank )?#?21$/ })).toBeVisible();
  await expect(page.getByText("1,000 USD", { exact: true })).toBeVisible();
  await expect(page.getByText("Showing 21-40 of 50", { exact: true })).toBeVisible();
  expect(queries).toHaveLength(1);
  await press(page, rankingPeriod(page).getByRole("button", { name: "Weekly ranking" }));
  await expect(rows.first()).toContainText("Largest");
  await expect(page.getByText("Showing 1-20 of 50", { exact: true })).toBeVisible();
  const halfway = rows.filter({ has: page.getByRole("cell", { name: "Halfway rounding", exact: true }) });
  await expect(halfway.getByRole("cell", { name: "10%", exact: true })).toBeVisible();
  await expect(halfway.getByRole("cell", { name: "9.9%", exact: true })).toHaveCount(0);

  const pageSize = page.getByRole("combobox", { name: "Rows per page" });
  await press(page, pageSize);
  await expect(page.getByRole("option")).toHaveText(["10", "20", "50"]);
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await expect(rows).toHaveCount(50);
  await expect(rows.last()).toContainText("Participant 50");
  await expect(rows.last().getByRole("cell", { name: "0 USD", exact: true })).toBeVisible();
});

test("loading, missing and empty snapshots are distinct; errors can retry to one and two ranked users", async ({ page }) => {
  let release!: () => void;
  const pause = new Promise<void>((resolve) => { release = resolve; });
  let monthlyAttempts = 0;
  const queries = await signIn(page, async (period, start) => {
    if (period === "day" && !start) {
      await pause;
      return { ...auditReport, entries: [], total_cost_amount: "0", refreshed_at: null };
    }
    if (period === "day") return { ...auditReport, period_start: "2026-07-23", period_end: "2026-07-24", refreshed_at: "2026-07-23T12:15:00Z",
      entries: [], total_cost_amount: "0", previous_period_start: null, next_period_start: "2026-07-24" };
    if (period === "week") return { ...auditReport, period, period_start: "2026-07-20", period_end: "2026-07-27",
      total_cost_amount: "0", entries: [entry(1, "Zero-cost priced entry", "0")] };
    expect(period).toBe("month");
    if (++monthlyAttempts === 1) return { error: "synthetic_snapshot_unavailable" };
    return { ...auditReport, period: "month", period_start: "2026-07-01", period_end: "2026-08-01", total_cost_amount: "100.98",
      entries: [entry(1, "Largest after retry", "100.98"), entry(2, "Tiny but non-zero", "0.000000000001")] };
  });
  await expect(page.getByRole("heading", { level: 1, name: "Spend leaderboard" })).toBeVisible();
  await expect.poll(() => queries.length).toBe(1);
  await expect(page.getByRole("status", { name: "Loading…", exact: true })).toBeVisible();
  await expect(page.getByText("No snapshot yet", { exact: true })).toHaveCount(0);
  await expect(page.getByText(/^0(?:\.00)? USD$/)).toHaveCount(0);
  await expect(tableRows(page)).toHaveCount(0);
  release();
  await expect(page.getByText("No snapshot yet", { exact: true })).toBeVisible();
  await expect(page.getByText("No priced requests in this period", { exact: true })).toHaveCount(0);
  await press(page, page.getByRole("button", { name: "Previous period" }));
  await expect(page.getByText("No priced requests in this period", { exact: true })).toBeVisible();
  await expect(page.getByText("No snapshot yet", { exact: true })).toHaveCount(0);
  await press(page, rankingPeriod(page).getByRole("button", { name: "Weekly ranking" }));
  await expect(tableRows(page)).toHaveCount(1);
  await expect(tableRows(page).first().getByRole("cell", { name: "—", exact: true })).toBeVisible();
  await expect(tableRows(page).first().getByText("0 USD", { exact: true })).toBeVisible();
  await press(page, rankingPeriod(page).getByRole("button", { name: "Monthly ranking" }));
  const retry = page.getByRole("button", { name: "Retry", exact: true });
  await expect(retry).toBeVisible();
  await expect(page.getByText("Zero-cost priced entry", { exact: true })).toHaveCount(0);
  await press(page, retry);
  await expect(tableRows(page)).toHaveCount(2);
  await expect(page.getByRole("status").filter({ hasText: /^July 2026$/ })).toBeFocused();
  expect(monthlyAttempts).toBe(2);
  const tiny = tableRows(page).last();
  await expect(tiny).toContainText("Tiny but non-zero");
  await expect(tiny.getByRole("cell", { name: "<0.1%", exact: true })).toBeVisible();
  await expect(tiny.getByRole("cell", { name: "<0.00001 USD", exact: true })).toBeVisible();
});

test("320px Chinese dark view wraps a long name and rounded USD amount without a hidden column", async ({ page }, info) => {
  await page.setViewportSize({ width: 320, height: 720 });
  const longName = "华中华南西南联合研发及长模型团队".repeat(5);
  const amount = "12345678912345679.123456";
  const report = { ...auditReport, total_cost_amount: amount, entries: [entry(1, longName, amount)] };
  await signIn(page, () => report, { locale: "zh-CN", theme: "dark" });
  await expect(page.locator("html")).toHaveClass(/\bdark\b/);
  await expect(page.getByRole("table")).toHaveCount(0);
  const row = rankedList(page, "用户排名").getByRole("listitem");
  await expect(row).toHaveCount(1);
  await expect(row.getByText(longName, { exact: true })).toBeVisible();
  await expect(row.getByText("12,345,678,912,345,679.12346 USD", { exact: true })).toBeVisible();
  await expect(row.getByText(/100%/)).toBeVisible();
  await expect(row.getByText(/6,800/)).toBeVisible();
  await expect(row.getByText("98.3M", { exact: true })).toBeVisible();
  const graphic = await row.ariaSnapshot();
  expect(graphic).toContain(longName);
  expect(graphic).toContain("100%");
  expect(graphic).not.toContain("progressbar");
  expect(await row.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(320);
  await screenshot(page, "after-320-dark-long-values", info);
});
