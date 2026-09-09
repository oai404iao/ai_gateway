import { expect, test } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";
import { SHARING_GROUP } from "../src/test/fixtures";

test("a sharing member sees private USD windows on mobile", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel("Email").fill("batch-user@example.test");
  await page.getByLabel("Password", { exact: true }).fill("mock-member-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).not.toHaveURL(/\/login/);
  await page.goto("/codex-sharing");
  await expect(page.getByRole("heading", { name: "My Codex sharing" })).toBeVisible();
  await expect(page.getByText("$6.90", { exact: true })).toBeVisible();
  await expect(page.getByText("Remaining: $6.90", { exact: true })).toBeInViewport();
  await expect(page.getByRole("columnheader", { name: "Provider used" })).toBeVisible();
  await page.getByRole("button", { name: "Refresh usage", exact: true }).click();
  await expect(page.getByText("$6.90", { exact: true })).toBeVisible();
  await page.goto("/admin/codex-sharing");
  await expect(page).toHaveURL(/\/account$/);
  await expect(page.getByRole("heading", { name: "Codex sharing", exact: true })).not.toBeVisible();
  await page.goto("/codex-sharing");
  await page.getByRole("button", { name: "Language", exact: true }).click();
  await page.getByRole("menuitemradio", { name: "简体中文", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(page.getByText("剩余: $6.90", { exact: true })).toBeInViewport();
  await expect(page.getByRole("columnheader", { name: "窗口", exact: true })).toBeVisible();
});

test("an administrator edits a fixed-seat budget with its ETag", async ({ page }) => {
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel("Email").fill("admin@example.com");
  await page.getByLabel("Password", { exact: true }).fill("mock-admin-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).not.toHaveURL(/\/login/);
  await page.goto("/admin/codex-sharing");
  await page.getByRole("link", { name: SHARING_GROUP.name }).click();
  const amount = page.getByLabel("Primary window total (USD)", { exact: true });
  await expect(amount).toHaveValue("20");
  await amount.fill("24");
  const saved = page.waitForResponse(response =>
    response.url().endsWith(`/codex-sharing-groups/${SHARING_GROUP.id}`)
    && response.request().method() === "PUT");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  expect((await saved).status()).toBe(200);
  await page.reload();
  await expect(amount).toHaveValue("24");
  await expect(page.getByRole("columnheader", { name: "Seat allowance" }).first()).toBeVisible();
});
