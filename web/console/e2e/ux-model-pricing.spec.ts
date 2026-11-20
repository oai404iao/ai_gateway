import { expect, test, type Locator, type Page } from "@playwright/test";
import type { ControlPlaneModel, ModelInput, TimeBillingMultiplier } from "../src/api/types";
import { E2E_MODEL, mockConsoleApi } from "./mock-api";

const pricingPath = `/admin/models/${E2E_MODEL.id}/pricing`;
const modelApiPath = `/console/v1/models/${E2E_MODEL.id}`;
const originalEtag = `"${E2E_MODEL.updated_at}"`;
const workspace = `/admin/models?model=${E2E_MODEL.id}&rule=00000000-0000-0000-0000-0000000000d1`;
const basePrice = (page: Page) => page.getByLabel(/^Input unit price/);
const savePricing = (page: Page) => page.getByRole("button", { name: "Save model pricing" });

test.use({ serviceWorkers: "block" });

async function mockPricing(page: Page, baseURL: string, extraWindows = 0) {
  const localOrigin = new URL(baseURL).origin;
  await page.route("**/*", (route) =>
    new URL(route.request().url()).origin === localOrigin ? route.continue() : route.abort(),
  );
  await mockConsoleApi(page);
  let model: ControlPlaneModel = {
    ...E2E_MODEL,
    advanced_billing: {
      ...E2E_MODEL.advanced_billing,
      time_multipliers: [
        {
          ...E2E_MODEL.advanced_billing.time_multipliers[0],
          weekdays: ["monday", "tuesday", "wednesday", "thursday", "friday"],
        },
        ...Array.from({ length: extraWindows }, (_, index): TimeBillingMultiplier => ({
          label: `Evening window ${index + 1}`,
          weekdays: ["monday"],
          start_time: `${16 + index}:00`,
          end_time: `${17 + index}:00`,
          multiplier: "1.2",
        })),
      ],
    },
  };
  let etag = originalEtag;
  const writes: { input: ModelInput; ifMatch: string | null }[] = [];
  await page.route(`**${modelApiPath}`, async (route) => {
    const method = route.request().method();
    if (method === "GET") return route.fulfill({ json: model, headers: { ETag: etag } });
    if (method !== "PUT") throw new Error(`Unexpected pricing request: ${method}`);
    const input = route.request().postDataJSON() as ModelInput;
    writes.push({ input, ifMatch: await route.request().headerValue("if-match") });
    model = {
      ...model,
      ...input,
      advanced_billing: { ...model.advanced_billing, ...input.advanced_billing },
    };
    etag = '"updated-pricing"';
    return route.fulfill({ json: { id: model.id, correlation_id: model.id } });
  });
  return writes;
}

async function signIn(page: Page) {
  await page.goto("/login");
  await page.locator('input[type="email"]').fill("admin@example.com");
  await page.locator('input[type="password"]').fill("synthetic-password");
  await page.locator('button[type="submit"]').click();
  await expect(page).toHaveURL(/\/account$/);
}

async function expectUnobscured(control: Locator) {
  await expect(control).toBeInViewport({ ratio: 1 });
  await expect.poll(() => control.evaluate((element) => {
    const { x, y, width, height } = element.getBoundingClientRect();
    return [1, height / 2, height - 1].every((offset) =>
      element.contains(document.elementFromPoint(x + width / 2, y + offset)),
    );
  })).toBe(true);
}

test("mobile can save base pricing from the first screen without opening the calculator", async ({ page, baseURL }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.setItem("ai-gateway-console.locale", "zh-CN");
    localStorage.setItem("console-theme", "light");
  });
  const writes = await mockPricing(page, baseURL!);
  await signIn(page);
  await page.goto(`${pricingPath}?returnTo=${encodeURIComponent(workspace)}`);

  const save = page.getByRole("button", { name: "保存模型价格" });
  const price = page.getByLabel(/^输入单价/);
  await expect(price).toHaveValue(E2E_MODEL.input_unit_price);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "倍率计算器" })).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByLabel(/^计算倍率/)).toBeHidden();
  await expectUnobscured(price);
  await expect(save).toBeDisabled();
  await price.fill("0.145000000001");
  await expect(page.getByText("有未保存的更改", { exact: true })).toBeInViewport();
  await expectUnobscured(save);

  await save.click();
  await expect(page).toHaveURL(new URL(workspace, baseURL).href);
  await expect(page.getByRole("heading", { name: E2E_MODEL.display_name })).toBeVisible();
  await expect(page.getByRole("group", { name: "操作路由" }).getByRole("button", { pressed: true })).toHaveCount(1);
  expect(writes).toHaveLength(1);
  expect(writes[0]).toMatchObject({
    ifMatch: originalEtag,
    input: { input_unit_price: "0.145000000001", advanced_billing: E2E_MODEL.advanced_billing },
  });
});

test("calculator scratch is local until explicitly filled and pricing discard and leave still protect the draft", async ({ page, baseURL }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const writes = await mockPricing(page, baseURL!);
  await signIn(page);
  await page.goto(`${pricingPath}?returnTo=${encodeURIComponent(workspace)}`);

  const disclosure = page.getByRole("button", { name: "Multiplier calculator" });
  const multiplier = page.getByLabel(/^Calculator multiplier/);
  const calculate = page.getByRole("button", { name: "Calculate and fill base prices" });
  await disclosure.press("Enter");
  await expect(disclosure).toHaveAttribute("aria-expanded", "true");
  await multiplier.fill("invalid");
  await calculate.click();
  await expect(multiplier).toHaveAttribute("aria-invalid", "true");
  await expect(basePrice(page)).toHaveValue("0.14");
  await multiplier.fill("1.25");
  await expect(basePrice(page)).toHaveValue("0.14");
  await expect(savePricing(page)).toBeDisabled();
  await multiplier.press("Enter");
  await expect(basePrice(page)).toHaveValue("0.175");
  await expect(savePricing(page)).toBeEnabled();
  expect(writes).toHaveLength(0);

  await page.getByRole("button", { name: "Discard pricing changes" }).click();
  await expect(basePrice(page)).toHaveValue("0.14");
  await expect(multiplier).toHaveValue("1");
  await expect(savePricing(page)).toBeDisabled();

  await multiplier.fill("invalid scratch");
  await disclosure.click();
  await expect(multiplier).toBeHidden();
  await basePrice(page).fill("0.2");
  const back = page.getByRole("link", { name: "Back to client models" });
  await back.click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();
  await expect(basePrice(page)).toHaveValue("0.2");
  expect(writes).toHaveLength(0);
  await savePricing(page).click();
  await expect(page).toHaveURL(new URL(workspace, baseURL).href);
  expect(writes).toHaveLength(1);
  expect(writes[0].input.input_unit_price).toBe("0.2");
});

test("short viewport exposes the first validation error, final UTC window and the same save action", async ({ page, baseURL }) => {
  await page.setViewportSize({ width: 844, height: 390 });
  await page.emulateMedia({ colorScheme: "dark" });
  const writes = await mockPricing(page, baseURL!, 3);
  await signIn(page);
  await page.goto(pricingPath);
  await expect(page.locator("html")).toHaveClass(/dark/);

  const price = basePrice(page);
  const lastStart = page.getByLabel("Start time (UTC)").last();
  const lastEnd = page.getByLabel("End time (UTC)").last();
  const save = savePricing(page);
  await expect(lastEnd).toHaveValue("19:00");
  await price.fill("invalid");
  await lastEnd.fill("");
  await expectUnobscured(save);
  await save.click();
  await expect(price).toBeFocused();
  await expect(price).toHaveAttribute("aria-invalid", "true");
  await expectUnobscured(price);
  expect(writes).toHaveLength(0);

  await price.fill("0.2");
  await save.click();
  await expect(lastEnd).toBeFocused();
  await expect(lastEnd).toHaveAttribute("aria-invalid", "true");
  await expectUnobscured(lastEnd);
  await page.keyboard.press("Shift+Tab");
  await expect(lastStart).toBeFocused();
  await expectUnobscured(lastStart);
  await page.keyboard.press("Tab");
  await expect(lastEnd).toBeFocused();
  await lastEnd.fill("19:30");
  await expectUnobscured(save);
  await save.click();
  await expect(save).toBeDisabled();
  await expect(page.getByText("Saved", { exact: true })).toBeVisible();
  expect(writes).toHaveLength(1);
  expect(writes[0].input.advanced_billing?.time_multipliers?.at(-1)?.end_time).toBe("19:30");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(844);
});

test("desktop keeps pricing and the calculator in two columns", async ({ page, baseURL }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockPricing(page, baseURL!);
  await signIn(page);
  await page.goto(pricingPath);

  const price = basePrice(page);
  const reference = page.getByLabel(/^Reference input unit price/);
  await expectUnobscured(price);
  await expectUnobscured(reference);
  expect((await reference.boundingBox())!.x).toBeGreaterThan((await price.boundingBox())!.x);
  await expect(page.getByRole("button", { name: "Multiplier calculator" })).toHaveCount(0);
  await expect(savePricing(page)).toHaveCount(1);
});
