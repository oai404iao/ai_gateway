import { expect, test, type Page } from "@playwright/test";
import { E2E_ROUTING_GROUP_ID, E2E_STANDARD_CHANNEL_ID, mockConsoleApi } from "./mock-api";

async function login(page: Page) {
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill("admin@example.com");
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/account$/);
}

test("an untouched account page can navigate without a discard prompt", async ({ page }) => {
  await login(page);
  await expect(page.getByLabel("Display name", { exact: true })).toHaveValue("Initial Admin");
  await expect(page.getByLabel("Current password", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Channel configuration", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Channel configuration" })).toBeVisible();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
});

test("browser Back, tabs and parent links share the same draft protection", async ({ page }) => {
  await login(page);
  const origin = `/admin/routing/channels?group=${E2E_ROUTING_GROUP_ID}`;
  await page.goto(origin);
  const channel = page.getByRole("link", { name: "Upstream A", exact: true });
  await channel.focus();
  await page.keyboard.press("Enter");
  await page.getByLabel("Name", { exact: true }).fill("Unsaved channel");
  await page.evaluate(() => window.history.back());
  const dialog = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("Unsaved channel");
  await page.getByRole("tab", { name: "Channel capabilities", exact: true }).click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.getByRole("link", { name: "New capability" })).toBeVisible();
  await page.getByRole("tab", { name: "Channel settings" }).click();
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("Upstream A");
  await page.getByLabel("Name", { exact: true }).fill("Another draft");
  await page.getByRole("link", { name: "Back to channels" }).click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page).toHaveURL(new RegExp(`${origin.replace("?", "\\?")}$`));
});

test("nested capability editing has one parent link and no competing batch editor", async ({ page }) => {
  await login(page);
  await page.setViewportSize({ width: 390, height: 844 });
  const origin = `/admin/routing/channels?group=${E2E_ROUTING_GROUP_ID}`;
  await page.goto(origin);
  await page.getByRole("link", { name: "Upstream A", exact: true }).click();
  await page.getByRole("tab", { name: "Channel capabilities", exact: true }).click();
  await page.getByRole("link", { name: "Chat Completions", exact: true }).click();
  await expect(page.getByRole("button", { name: "Save capability" })).toBeVisible();
  await expect(page.getByRole("link", { name: /^Back to/ })).toHaveCount(1);
  await expect(page.getByRole("button", { name: /Batch edit capabilities/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Delete channel" })).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  await page.getByRole("link", { name: "Back to capabilities" }).click();
  await expect(page.getByRole("button", { name: /Batch edit capabilities/ })).toBeVisible();
  await page.getByRole("link", { name: "Back to channels" }).click();
  await expect(page).toHaveURL(new RegExp(`${origin.replace("?", "\\?")}$`));
});

test("direct detail entry has a safe parent and legacy lists retain their filters", async ({ page }) => {
  await login(page);
  await page.goto(`/admin/routing/logical-channels/${E2E_STANDARD_CHANNEL_ID}?returnTo=%2F%2Fevil.test`);
  await expect(page.getByRole("link", { name: "Back to channels" }))
    .toHaveAttribute("href", "/admin/routing/channels");
  await page.getByRole("link", { name: "Back to channels" }).click();
  await page.goto(`/admin/routing/logical-channels?group=${E2E_ROUTING_GROUP_ID}`);
  await expect(page).toHaveURL(new RegExp(`/admin/routing/channels\\?group=${E2E_ROUTING_GROUP_ID}$`));
  await expect(page.getByRole("link", { name: "Upstream A", exact: true })).toBeVisible();
  await expect(page.getByRole("link", { name: "Personal Plus", exact: true })).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
});
