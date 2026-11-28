import { expect, test, type Locator } from "@playwright/test";
import { E2E_MODEL, E2E_USER, mockConsoleApi } from "./mock-api";

const identifier = "abcdefghijklmnopqrstuvwxyz".repeat(7);

test.use({ viewport: { width: 320, height: 780 } });
test.beforeEach(async ({ page }) => {
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill("admin@example.com");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
});

async function expectInside(control: Locator, ancestor: string) {
  await expect(control).toBeVisible();
  const bounds = await control.evaluate((node, selector) => {
    const parent = node.closest(selector)!.getBoundingClientRect();
    const rect = node.getBoundingClientRect();
    return { left: rect.left - parent.left, right: parent.right - rect.right };
  }, ancestor);
  expect(Math.min(bounds.left, bounds.right)).toBeGreaterThanOrEqual(-1);
}

test("model details wrap long IDs without widening or clipping the editor", async ({ page }) => {
  await page.route((url) => url.pathname === `/console/v1/models/${E2E_MODEL.id}`, (route) =>
    route.fulfill({ headers: { ETag: '"model"' }, json: { ...E2E_MODEL, source_model_id: identifier } }),
  );
  await page.goto(`/admin/models/${E2E_MODEL.id}`);
  const id = page.getByText(identifier, { exact: true });
  await expectInside(id, '[data-slot="card-header"]');
  expect(await id.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(true);

  const provider = page.getByLabel("Provider name");
  await provider.fill(identifier);
  await provider.press("End");
  await expectInside(provider, "form");
  await expect.poll(() => provider.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
  await provider.press("Shift+ArrowLeft");
  await provider.press("Backspace");
  await expect(provider).toHaveValue(identifier.slice(0, -1));

  const payload = page.getByLabel("Source payload (JSON, optional)");
  const json = JSON.stringify({ model: identifier.repeat(3) }, null, 2);
  await payload.fill(json);
  await expectInside(payload, "fieldset");
  expect(await payload.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  await payload.press("Control+End");
  await payload.press("Enter");
  await expect(payload).toHaveValue(json + "\n");
});

test("request-ID actions and long user filters fit a narrow form", async ({ page }) => {
  await page.route((url) => url.pathname === "/console/v1/users", (route) =>
    route.fulfill({ json: { items: [{ ...E2E_USER, display_name: identifier }], next_cursor: null, total: 1 } }),
  );
  await page.route((url) => url.pathname === `/console/v1/users/${E2E_USER.id}`, (route) =>
    route.fulfill({
      headers: { ETag: `"${E2E_USER.updated_at}"` },
      json: { ...E2E_USER, display_name: identifier },
    }),
  );
  await page.goto("/admin/request-logs");
  const search = page.getByRole("searchbox", { name: "Find request ID" });
  await search.fill("00000000-0000-0000-0000-0000000000f1");
  const lookup = page.locator("form").filter({ has: search }).getByRole("button", { name: "View details" });
  await expectInside(lookup, "form");
  await expectInside(lookup, '[data-slot="input-group"]');
  await expectInside(search, '[data-slot="input-group"]');
  expect(await search.evaluate((el) => el.getBoundingClientRect().right)).toBeLessThanOrEqual(
    await lookup.evaluate((el) => el.getBoundingClientRect().left + 1),
  );

  await page.getByRole("button", { name: /^Filters \(/ }).click();
  const user = page.getByRole("combobox", { name: "User", exact: true });
  await user.click();
  const option = page.getByRole("option", { name: `${identifier} · ${E2E_USER.email}` });
  await expect(option).toBeVisible();
  expect(await option.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  await option.click();
  await expect(user).toHaveValue(new RegExp(identifier));
  await expectInside(user, "form");
  expect(
    await user.evaluate((el) => el.getBoundingClientRect().right <= el.closest("form")!.getBoundingClientRect().right + 1),
  ).toBe(true);
});
