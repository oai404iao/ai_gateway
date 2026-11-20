import { expect, test, type Page } from "@playwright/test";
import type { ModelSyncPreview, ModelSyncPreviewModel, ModelSyncSelection } from "../src/api/types";
import { mockConsoleApi } from "./mock-api";

const catalog: ModelSyncPreviewModel[] = Array.from({ length: 60 }, (_, index) => {
  const suffix = String(index + 1).padStart(3, "0");
  return {
    provider_id: index < 30 ? "atlas-cloud" : "birch-cloud",
    provider_name: index < 30 ? "Atlas Labs" : "Birch AI",
    model_id: `example/billing-${suffix}`,
    display_name: `${index === 54 ? "Special Billing" : "Catalog Model"} ${suffix}`,
    input_unit_price: "0.01",
    cached_input_unit_price: "0.001",
    cache_write_unit_price: "0.02",
    output_unit_price: "0.04",
    advanced_billing: { long_context_tiers: [], request_multipliers: [] },
    action: index === 0 ? "price_update" : "import",
    preview_token: `${suffix}${"a".repeat(61)}`,
  };
});

async function openPriceSync(page: Page) {
  await mockConsoleApi(page);
  const previews: string[][] = [];
  const applied: ModelSyncSelection[][] = [];
  await page.route("**/console/v1/catalog/models/sync/preview", (route) => {
    expect(route.request().method()).toBe("POST");
    const { provider_ids } = route.request().postDataJSON();
    previews.push(provider_ids);
    return route.fulfill({ json: {
      fetched_at: "2026-08-24T00:00:00.000Z",
      models: provider_ids.includes("none") ? [] : catalog,
      excluded_missing_prices: 0,
      excluded_invalid_models: 0,
      excluded_oversized_metadata: 0,
    } satisfies ModelSyncPreview });
  });
  await page.route("**/console/v1/catalog/models/import", (route) => {
    expect(route.request().method()).toBe("POST");
    applied.push(route.request().postDataJSON().selections);
    return route.fulfill({ json: {
      model_count: 2, imported_count: 1, updated_count: 1,
      correlation_id: "00000000-0000-0000-0000-000000000041",
    } });
  });
  await page.goto("/login");
  await page.locator('input[type="email"]').fill("admin@example.com");
  await page.locator('input[type="password"]').fill("synthetic-password");
  await page.locator('button[type="submit"]').click();
  await expect(page).toHaveURL(/\/account$/);
  await page.goto("/admin/models?view=prices");
  return { previews, applied };
}

test("price sync searches the full preview locally and applies the exact selection across filters", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const { previews, applied } = await openPriceSync(page);
  const search = page.getByRole("searchbox", { name: "Search name, model ID, or provider" });
  const fetch = page.getByRole("button", { name: "Fetch preview" });
  const row = (item: ModelSyncPreviewModel) => page.getByRole("checkbox", { name: `Select ${item.model_id}` });
  await expect(search).toHaveCount(0);
  await fetch.click();
  await expect(page.getByText("60 catalog models.")).toBeVisible();
  await row(catalog[0]).click();
  await expect(row(catalog[54])).toHaveCount(0);
  await page.getByRole("button", { name: "Go to page 3" }).click();
  await expect(page.getByText("Showing 41-60 of 60")).toBeVisible();
  const searchRequests: string[] = [];
  const recordSearchRequest = (request: { url: () => string }) => {
    if (request.url().includes("/console/v1/")) searchRequests.push(request.url());
  };
  page.on("request", recordSearchRequest);
  await search.fill("  bIRCh aI  ");
  await expect(page.getByText("30 of 60 catalog models match.")).toBeVisible();
  await expect(page.getByText("Showing 1-20 of 30")).toBeVisible();
  await expect(row(catalog[30])).toBeVisible();
  await search.fill("BIRCH-CLOUD");
  await expect(page.getByText("30 of 60 catalog models match.")).toBeVisible();
  await search.fill("BILLING-055");
  await expect(row(catalog[54])).toBeVisible();
  await row(catalog[54]).click();
  await search.fill("sPEcIaL BiLlInG");
  await expect(page.getByText("1 of 60 catalog models match.")).toBeVisible();
  await expect(row(catalog[54])).toBeChecked();
  await expect(page.getByText("Selected models outside these results: 1")).toBeVisible();
  await expect(page.getByRole("button", { name: "Apply selected (2)" })).toBeEnabled();
  await search.scrollIntoViewIfNeeded();
  await testInfo.attach("desktop filtered results", { body: await page.screenshot(), contentType: "image/png" });
  expect(searchRequests).toEqual([]);
  expect(previews).toEqual([[]]);
  expect(applied).toEqual([]);
  page.off("request", recordSearchRequest);
  await page.getByRole("button", { name: "Apply selected (2)" }).click();
  await expect(page.getByRole("button", { name: "Apply selected (0)" })).toBeDisabled();
  await expect(row(catalog[54])).not.toBeChecked();
  await expect(search).toHaveValue("sPEcIaL BiLlInG");
  expect(previews).toEqual([[], []]);
  expect(applied).toEqual([[catalog[0], catalog[54]].map(({ provider_id, model_id, preview_token }) => ({
    provider_id, model_id, preview_token,
  }))]);
});

test("mobile search can clear zero matches without fetching, and distinguishes an empty preview", async ({ page }, testInfo) => {
  await page.addInitScript(() => localStorage.setItem("ai-gateway-console.locale", "zh-CN"));
  await page.setViewportSize({ width: 390, height: 844 });
  const { previews } = await openPriceSync(page);
  const fetch = page.getByRole("button", { name: "获取预览" });
  await fetch.click();
  const search = page.getByRole("searchbox", { name: "搜索名称、模型 ID 或供应商" });
  await expect(search).toBeInViewport({ ratio: 1 });
  await search.fill("unknown-model");
  await expect(page.getByText("60 个目录模型中有 0 个匹配。")).toBeVisible();
  await expect(page.getByText("没有匹配的模型")).toBeVisible();
  await testInfo.attach("mobile no matches", { body: await page.screenshot(), contentType: "image/png" });
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "清除搜索" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(search).toBeFocused();
  await expect(search).toHaveValue("");
  await expect(page.getByText("60 个目录模型。")).toBeVisible();
  expect(previews).toEqual([[]]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.locator("#providers").fill("none");
  await fetch.click();
  await expect(page.getByText("没有目录模型")).toBeVisible();
  await expect(search).toHaveCount(0);
  expect(previews).toEqual([[], ["none"]]);
});
