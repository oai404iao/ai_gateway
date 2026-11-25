import { expect, test } from "@playwright/test";
import { E2E_USER, mockConsoleApi } from "./mock-api";

test("overview metrics and interval controls work while the plot is loading", async ({ page }) => {
  await mockConsoleApi(page);
  const { promise: held, resolve: release } = Promise.withResolvers<void>();
  await page.route("**/usage-trend-plot.tsx*", async (route) => {
    await held;
    await route.continue();
  });
  try {
    await page.goto("/login");
    await page.getByLabel("Email", { exact: true }).fill(E2E_USER.email);
    await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    const trends = page.getByRole("region", { name: "Usage trends" });
    await expect(trends.getByRole("status", { name: "Loading…" })).toBeVisible();
    const tokens = trends.getByRole("button", { name: /^Total tokens/ });
    await tokens.click();
    await expect(tokens).toHaveAttribute("aria-pressed", "true");
    await trends.getByRole("button", { name: "Previous interval" }).click();
    await expect(trends.getByText("16M Tokens")).toBeVisible();
    await expect(trends.getByRole("application")).toHaveCount(0);
  } finally {
    release();
  }
  await expect(page.getByRole("application")).toBeVisible();
});
