import { expect, test, type Locator, type Page } from "@playwright/test";
import { E2E_SYSTEM_REQUEST_LOG, E2E_USER, mockConsoleApi } from "./mock-api";

async function login(page: Page, email = "admin@example.com") {
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill(email);
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
}

function dock(page: Page) {
  return page.getByRole("navigation", { name: "Mobile navigation" });
}

async function expectFirstViewport(locator: Locator) {
  await expect(locator).toBeVisible();
  expect(await locator.evaluate((node) => {
    const bounds = node.getBoundingClientRect();
    const toolbar = document.querySelector("main > header")?.getBoundingClientRect();
    return bounds.top >= (toolbar?.bottom ?? 0) && bounds.bottom <= innerHeight &&
      bounds.left >= 0 && bounds.right <= innerWidth;
  })).toBe(true);
}

test.describe("mobile navigation", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("opens management and settings from the page finder; Escape returns to the opener", async ({ page }) => {
    await login(page);
    await page.goto("/admin/models");
    await expect(page.getByRole("heading", { level: 1, name: "Model configuration" })).toBeVisible();
    const toggle = dock(page).getByRole("button", { name: "More pages" });
    const finder = page.getByRole("dialog", { name: "Find page" });
    await toggle.click();
    await expect(finder).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(finder).toBeHidden();
    await expect(toggle).toBeFocused();
    await expect(page).toHaveURL(/\/admin\/models$/);

    await toggle.press("Enter");
    await finder.getByRole("combobox").fill("/admin/users");
    await finder.getByRole("option", { name: /Users/ }).click();
    await expect(page).toHaveURL(/\/admin\/users$/);
    await expect(finder).toBeHidden();
    const heading = page.getByRole("heading", { level: 1, name: "Users" });
    await expect(page.getByRole("main")).toBeFocused();
    await expectFirstViewport(heading);
    const user = page.getByRole("link", { name: E2E_USER.display_name });
    await user.click();
    await expect(page).toHaveURL(new RegExp(`/admin/users/${E2E_USER.id}`));
    await expect(page.getByRole("heading", { level: 1, name: E2E_USER.display_name })).toBeVisible();

    await toggle.click();
    await finder.getByRole("combobox").fill("/admin/system/upstream");
    await finder.getByRole("option", { name: /Upstream timeouts/ }).click();
    await expect(page).toHaveURL(/\/admin\/system\/upstream$/);
    await expect(finder).toBeHidden();
    await expect(page.getByRole("heading", { level: 1, name: /Upstream timeouts/ })).toBeVisible();
    await expect(page.getByRole("main")).toBeFocused();
  });

  test("canceling a dirty navigation keeps the draft; confirming a dock link moves to its content", async ({ page }) => {
    await login(page);
    await page.goto("/admin/system/upstream");
    const timeout = page.getByLabel("Connect timeout (seconds)");
    await timeout.fill("23");
    const destination = dock(page).getByRole("link", { name: "Request logs" });
    await destination.click();
    const guard = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
    await guard.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(guard).toBeHidden();
    await expect(dock(page)).toBeVisible();
    await expect(destination).toBeFocused();
    await expect(page).toHaveURL(/\/admin\/system\/upstream$/);
    await expect(timeout).toHaveValue("23");

    await destination.press("Enter");
    await guard.getByRole("button", { name: "Discard changes" }).click();
    await expect(page).toHaveURL(/\/usage\/request-logs$/);
    await expect(page.getByRole("heading", { level: 1, name: /Request logs/i })).toBeVisible();
    await expect(page.getByRole("main")).toBeFocused();
  });
});

test.describe("mobile request logs", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  for (const scope of ["personal", "system"] as const) {
    test(`${scope}: recent rows, applied filters, and keyboard details`, async ({ page }) => {
      const personal = scope === "personal";
      await login(page, personal ? E2E_USER.email : undefined);
      const path = personal ? "/usage/request-logs" : "/admin/request-logs";
      const api = personal ? "/console/v1/me/request-logs" : "/console/v1/request-logs";
      const first = { ...E2E_SYSTEM_REQUEST_LOG, billed_at: null, client_model: "Recent request" };
      const second = {
        ...E2E_SYSTEM_REQUEST_LOG,
        id: "00000000-0000-0000-0000-0000000000f9",
        client_model: "Earlier request",
        outcome: "succeeded",
        started_at: "2026-07-27T10:00:00.000Z",
      };
      const queries: URLSearchParams[] = [];
      await page.route((url) => url.pathname === api, async (route) => {
        expect(route.request().method()).toBe("GET");
        const params = new URL(route.request().url()).searchParams;
        queries.push(params);
        const rows = [first, second].filter((row) =>
          (!params.has("outcome") || row.outcome === params.get("outcome")) &&
          (!params.has("billed") || String(row.billed_at !== null) === params.get("billed")),
        );
        await route.fulfill({ json: rows });
      });
      await page.route((url) => url.pathname === `${api}/${first.id}`, async (route) => {
        expect(route.request().method()).toBe("GET");
        await route.fulfill({ json: first });
      });
      await page.goto(path);
      const row = page.getByRole("row", { name: /Recent request/ });
      const detailButton = row.getByRole("button", { name: "View details" });
      const earlier = page.getByRole("row", { name: /Earlier request/ });
      const filters = page.getByRole("button", { name: /^Filters \(/ });
      const outcome = page.getByRole("combobox", { name: "Outcome" });

      await expect(filters).toHaveText("Filters (0)");
      await expect(filters).toHaveAttribute("aria-expanded", "false");
      await expect(outcome).toBeHidden();
      await expectFirstViewport(row);
      await expectFirstViewport(detailButton);
      await expect(row.locator("time")).toHaveAttribute("datetime", first.started_at);
      await expect(row).toContainText("Failed");
      expect(queries.map((params) => params.toString())).toEqual(["limit=50"]);

      await filters.click();
      await expect(page.getByRole("combobox", { name: "User", exact: true })).toHaveCount(personal ? 0 : 1);
      await outcome.click();
      await page.getByRole("option", { name: "Failed", exact: true }).click();
      await page.getByRole("combobox", { name: "Billing", exact: true }).click();
      await page.getByRole("option", { name: "Unbilled", exact: true }).click();
      await filters.click();
      await expect(filters).toHaveText("Filters (0)");
      await expectFirstViewport(earlier);
      expect(queries).toHaveLength(1);

      await filters.click();
      await expect(outcome).toContainText("Failed");
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await expect(filters).toHaveAttribute("aria-expanded", "false");
      await expect(filters).toBeFocused();
      await expect(filters).toHaveText("Filters (2)");
      const summary = page.getByRole("list", { name: "Applied filters" });
      await expect(summary.getByRole("listitem")).toHaveText(["Outcome: Failed", "Billing: Unbilled"]);
      await expect(row).toBeVisible();
      await expect(earlier).toHaveCount(0);
      expect(queries.map((params) => Object.fromEntries(params))).toEqual([
        { limit: "50" },
        { limit: "50", outcome: "failed", billed: "false" },
      ]);

      await page.getByRole("button", { name: "Clear filters" }).click();
      await expect(filters).toBeFocused();
      await expect(filters).toHaveText("Filters (0)");
      await expect(summary).toHaveCount(0);
      await expect(earlier).toBeVisible();

      await detailButton.press("Enter");
      const detail = page.getByRole("dialog", { name: "Request log" });
      await expect(detail.getByLabel("Request ID", { exact: true })).toHaveText(first.id);
      await expect(detail.getByText(first.error_summary)).toBeVisible();
      await expect(detail.getByText(first.channel_name, { exact: true })).toHaveCount(personal ? 0 : 1);
      await expect(detail.getByText(first.user_name, { exact: true })).toHaveCount(personal ? 0 : 1);
      await page.keyboard.press("Escape");
      await expect(detail).toBeHidden();
      await expect(detailButton).toBeFocused();
      await expectFirstViewport(detailButton);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);
    });
  }
});

test("desktop request logs retain the comparison table without growing for long identifiers", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ colorScheme: "dark" });
  await login(page);
  const longModel = "synthetic-provider/" + "unbroken-model-identifier".repeat(16);
  await page.route((url) => url.pathname === "/console/v1/request-logs", (route) =>
    route.fulfill({ json: [{ ...E2E_SYSTEM_REQUEST_LOG, client_model: longModel,
      input_tokens: 8e18, cached_input_tokens: 4e18, output_tokens: 7e18, reasoning_tokens: 3e18 }] }),
  );
  await page.goto("/admin/request-logs");
  await expect(page.locator("html")).toHaveClass(/\bdark\b/);
  await page.getByRole("button", { name: "Filters (0)" }).click();
  await expect(page.getByRole("combobox", { name: "User", exact: true })).toBeVisible();
  const table = page.getByRole("table");
  await expect(table.getByRole("columnheader", { name: "Operation / protocol" })).toBeVisible();
  await expect(table.getByRole("columnheader", { name: "Channel", exact: true })).toBeVisible();
  await expect(table.getByRole("columnheader", { name: "User", exact: true })).toBeVisible();
  const cell = table.getByRole("cell").filter({ hasText: longModel });
  await expect(cell).toBeVisible();
  expect(await cell.evaluate((node) => node.getBoundingClientRect().width)).toBeLessThan(226);
  expect(await cell.evaluate((node) => node.getBoundingClientRect().height)).toBeLessThan(120);
  expect(await table.locator("td").evaluateAll(cells => cells.every(cell => cell.scrollWidth <= cell.clientWidth + 1))).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(1440);
  await table.getByRole("button", { name: "View details" }).click();
  const inspector = page.getByRole("complementary", { name: "Request log" });
  await expect(inspector).toBeVisible();
  await expect(table).toBeVisible();
  await inspector.getByRole("button", { name: "Close inspector" }).press("Escape");
  await expect(inspector).toBeHidden();
  await expect(table.getByRole("button", { name: "View details" })).toBeFocused();
});

function tableLayout(table: Locator) {
  return table.evaluate((node: HTMLTableElement) => {
    const scroller = node.parentElement!;
    const action = node.tBodies[0].rows[0].cells[node.rows[0].cells.length - 1];
    const button = action.querySelector("button")!.getBoundingClientRect();
    const viewport = scroller.getBoundingClientRect();
    return {
      available: scroller.clientWidth,
      width: node.offsetWidth,
      overflow: scroller.scrollWidth - scroller.clientWidth,
      action: action.offsetWidth,
      actionAtEdge: Math.abs(viewport.right - action.getBoundingClientRect().right) <= 1 &&
        button.left >= viewport.left && button.right <= viewport.right,
    };
  });
}

test("system log columns share the table width and keep the action reachable in the split inspector", async ({ page }) => {
  await login(page);
  await page.goto("/admin/request-logs");
  const table = page.getByRole("table");
  const details = table.getByRole("button", { name: "View details" });
  const model = table.getByRole("columnheader", { name: "Model", exact: true });
  const modelWidth = () => model.evaluate(node => node.getBoundingClientRect().width);
  const chooser = page.getByRole("button", { name: "Display columns" });
  await expect(details).toBeVisible();
  for (const width of [1372, 1440, 1552, 1618]) {
    await page.setViewportSize({ width, height: 900 });
    const layout = await tableLayout(table);
    expect(layout, `viewport ${width}`).toMatchObject({ width: layout.available, overflow: 0, action: 44, actionAtEdge: true });
  }

  const full = await tableLayout(table);
  const before = await modelWidth();
  await chooser.click();
  await page.getByRole("menuitemcheckbox", { name: "Channel group", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(table.getByRole("columnheader", { name: "Channel group", exact: true })).toHaveCount(0);
  expect(await modelWidth()).toBeGreaterThan(before);
  expect(await tableLayout(table)).toEqual(full);

  await details.press("Enter");
  const inspector = page.getByRole("complementary", { name: "Request log" });
  const close = inspector.getByRole("button", { name: "Close inspector" });
  await expect(close).toBeFocused();
  const split = await tableLayout(table);
  expect(split.available).toBeLessThan(full.available);
  expect(split.overflow).toBeGreaterThan(0);
  expect(split.actionAtEdge).toBe(true);
  const scroller = table.locator("..");
  await scroller.evaluate(node => { node.scrollLeft = node.scrollWidth; });
  expect(await tableLayout(table)).toMatchObject({ action: full.action, actionAtEdge: true });
  await chooser.click();
  for (const name of ["Channel", "User", "Cost"]) {
    await page.getByRole("menuitemcheckbox", { name, exact: true }).click();
  }
  await page.keyboard.press("Escape");
  await expect(table.getByRole("columnheader")).toHaveCount(7);
  expect(await tableLayout(table)).toMatchObject({ available: split.available, width: split.available, overflow: 0, actionAtEdge: true });

  await page.setViewportSize({ width: 820, height: 900 });
  const inspectorRect = (await inspector.boundingBox())!;
  const tableRect = (await table.boundingBox())!;
  expect(tableRect.y).toBeGreaterThanOrEqual(inspectorRect.y + inspectorRect.height);
  expect(await tableLayout(table)).toMatchObject({ actionAtEdge: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(820);
  await close.press("Escape");
  await expect(details).toBeFocused();
});

test("ordinary Chinese system logs fit without horizontal scrolling", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1618, height: 900 });
  await page.emulateMedia({ colorScheme: "dark" });
  await login(page);
  await page.addInitScript(() => localStorage.setItem("ai-gateway-console.locale", "zh-CN"));
  await page.route(url => url.pathname === "/console/v1/request-logs", route => route.fulfill({
    json: [{ ...E2E_SYSTEM_REQUEST_LOG, client_model: "gpt-5.6-terra", upstream_model: "gpt-5.6-terra", channel_name: "JP_1", user_name: "系统管理员", channel_group_name: "测试 plus" }],
  }));
  await page.goto("/admin/request-logs");
  const table = page.getByRole("table");
  await expect(table.getByRole("button", { name: "查看详情" })).toBeVisible();
  expect(await tableLayout(table)).toMatchObject({ overflow: 0, actionAtEdge: true });
  const path = testInfo.outputPath("system-logs.png");
  await table.screenshot({ path });
  await testInfo.attach("System log table", { path, contentType: "image/png" });
});

test("opening the stacked inspector from a later row brings its controls into view", async ({ page }) => {
  await page.setViewportSize({ width: 820, height: 650 });
  await login(page);
  const rows = Array.from({ length: 18 }, (_, i) => ({
    ...E2E_SYSTEM_REQUEST_LOG, id: `00000000-0000-0000-0000-${String(i + 1).padStart(12, "0")}`,
  }));
  await page.route(url => url.pathname === "/console/v1/request-logs", route => route.fulfill({ json: rows }));
  await page.route(url => url.pathname === `/console/v1/request-logs/${rows.at(-1)!.id}`, route => route.fulfill({ json: rows.at(-1) }));
  await page.goto("/admin/request-logs");
  const opener = page.getByRole("table").getByRole("button", { name: "View details" }).last();
  await opener.scrollIntoViewIfNeeded();
  await opener.press("Enter");
  const close = page.getByRole("button", { name: "Close inspector" });
  await expect(close).toBeFocused();
  await expect(close).toBeInViewport({ ratio: 1 });
  await close.press("Escape");
  await expect(opener).toBeFocused();
  await expect(opener).toBeInViewport({ ratio: 1 });
});
