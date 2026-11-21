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
    route.fulfill({ json: [{ ...E2E_SYSTEM_REQUEST_LOG, client_model: longModel }] }),
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
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(1440);
  await table.getByRole("button", { name: "View details" }).click();
  const inspector = page.getByRole("complementary", { name: "Request log" });
  await expect(inspector).toBeVisible();
  await expect(table).toBeVisible();
  await inspector.getByRole("button", { name: "Close inspector" }).press("Escape");
  await expect(inspector).toBeHidden();
  await expect(table.getByRole("button", { name: "View details" })).toBeFocused();
});
