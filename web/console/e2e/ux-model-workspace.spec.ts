import { expect, test, type Locator, type Page } from "@playwright/test";
import type { ControlPlaneModel } from "../src/api/types";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL, OPERATION_RULE, ROUTING_PROFILE } from "../src/test/fixtures";
import { E2E_MODEL, mockConsoleApi } from "./mock-api";

const directory: ControlPlaneModel[] = Array.from({ length: 83 }, (_, index) => {
  const number = index + 1;
  const suffix = String(number).padStart(3, "0");
  const provider = ["Atlas", "Beryl", "Cedar", "Delta"][Math.floor(index / 21)];
  const family = number % 2 ? "Reasoning" : "Chat";
  const name = `${provider} ${family} ${suffix}`;
  return {
    ...E2E_MODEL,
    id: `10000000-0000-0000-0000-${suffix.padStart(12, "0")}`,
    provider_name: provider,
    display_name: number === 81 ? "Delta Chat 080" : name,
    source_model_id: `${provider.toLowerCase()}/mock-${family.toLowerCase()}-${suffix}${number === 80 ? `-${"long-id-".repeat(8)}end` : ""}`,
    enabled: number % 9 !== 0,
    price_unit_tokens: number === 80 ? 1_001 : number % 6 === 0 ? 1000 : 1000000,
    input_unit_price: number === 80 ? "0.123456789123456789" : E2E_MODEL.input_unit_price,
  };
}).concat(E2E_MODEL);
const model = directory[79];
const operations = [
  {
    ...OPERATION_RULE,
    routing_tiers: Array.from({ length: 5 }, (_, priority) => ({
      ...OPERATION_RULE.routing_tiers[0],
      priority,
      candidates: ["gpt-5", "gpt-backup"].map((upstream_model) => ({
        capability_id: CHANNEL_CAPABILITY.id, upstream_model, weight: 1,
      })),
    })),
  },
  {
    ...OPERATION_RULE,
    id: "00000000-0000-0000-0000-000000001803",
    operation: "chat_completion" as const,
    enabled: false,
    routing_tiers: [],
  },
];
const profile = { ...ROUTING_PROFILE, model_id: model.id, client_model: model.source_model_id, model_display_name: model.display_name };
const deepLink = `/admin/models?model=${model.id}&rule=${operations[0].id}`;

async function prepare(page: Page) {
  const writes: string[] = [];
  await mockConsoleApi(page);
  await page.route("**/console/v1/**", (route) => {
    const method = route.request().method();
    if (method !== "GET" && !new URL(route.request().url()).pathname.includes("/auth/")) {
      writes.push(`${method} ${route.request().url()}`);
    }
    return route.fallback();
  });
  for (const [path, json] of Object.entries({
    models: directory,
    "routing/logical-channels": [LOGICAL_CHANNEL],
    "routing/capabilities": [{ ...CHANNEL_CAPABILITY, settings: {
      ...CHANNEL_CAPABILITY.settings, available_models: ["gpt-5", "gpt-backup"],
    } }],
    "routing/profiles": [profile],
    "routing/operation-rules": operations,
    [`models/${model.id}`]: model,
  })) {
    await page.route(`**/console/v1/${path}`, async (route) => {
      expect(route.request().method()).toBe("GET");
      await route.fulfill({ json, headers: { ETag: `"${model.updated_at}"` } });
    });
  }
  for (const rule of operations) {
    await page.route(`**/console/v1/routing/operation-rules/${rule.id}`, async (route) => {
      expect(route.request().method()).toBe("GET");
      await route.fulfill({ json: rule, headers: { ETag: `"${rule.updated_at}"` } });
    });
  }
  await page.goto("/login");
  await page.locator("input[type=email]").fill("admin@example.com");
  await page.locator("input[type=password]").fill("synthetic-password");
  await page.locator("button[type=submit]").click();
  await expect(page).toHaveURL("/");
  return writes;
}

const row = (page: Page, item = model) => page.getByRole("button").filter({
  has: page.getByText(item.source_model_id, { exact: true }),
});
const search = (page: Page) => page.getByRole("searchbox", { name: "Search name, model ID, or provider" });
const heading = (page: Page) => page.getByRole("heading", { name: model.display_name, exact: true });

async function panePosition(locator: Locator) {
  return locator.evaluate((element) => {
    let ancestor = element.parentElement;
    while (ancestor && getComputedStyle(ancestor).overflowY !== "auto") ancestor = ancestor.parentElement;
    if (!ancestor) throw new Error("No scroll pane");
    return {
      scrollTop: ancestor.scrollTop,
      offset: element.getBoundingClientRect().top - ancestor.getBoundingClientRect().top,
    };
  });
}

for (const [width, height] of [[1440, 900], [1280, 720]]) {
  test(`deep selection keeps both independent panes on screen at ${width}×${height}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height });
    const writes = await prepare(page);
    await page.goto("/admin/models");
    await expect(search(page)).toBeVisible();
    await page.getByRole("heading", { name: "Atlas" }).hover();
    await page.mouse.wheel(0, (await panePosition(row(page))).offset - 20);
    await expect(row(page)).toBeInViewport({ ratio: 1 });
    const { scrollTop: listY } = await panePosition(row(page));
    expect(listY).toBeGreaterThan(1000);
    const windowY = await page.evaluate(() => scrollY);
    await row(page).click();
    await expect(row(page)).toBeFocused();
    await expect(row(page)).toHaveAttribute("aria-current", "true");
    await expect(row(page)).toBeInViewport({ ratio: 1 });
    await expect(heading(page)).toBeInViewport({ ratio: 1 });
    expect((await heading(page).boundingBox())!.y).toBeGreaterThan(56);
    expect(await page.evaluate(() => scrollY)).toBe(windowY);
    expect((await panePosition(row(page))).scrollTop).toBe(listY);
    const detail = page.getByRole("region", { name: "Operation routing" });
    await expect(detail.getByText("0.12346")).toBeVisible();
    await expect(detail.getByText("Base prices · USD / 1K (1,001) tokens")).toBeVisible();
    await expect(row(page).getByText("0.12346")).toBeVisible();
    const enterEditor = page.getByRole("button", { name: "Go to selected model configuration" });
    await page.keyboard.press("Tab");
    await expect(enterEditor).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(row(page)).toBeFocused();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await expect(heading(page)).toBeFocused();
    await testInfo.attach("desktop deep selection", { body: await page.screenshot(), contentType: "image/png" });
    await detail.getByRole("button", { name: /Responses/ }).click();
    await detail.getByRole("button", { name: "Inspect tier 5", exact: true }).click();
    const bottom = detail.getByRole("spinbutton", { name: "Weight for tier 5 row 2" });
    await expect(bottom).toBeAttached();
    await detail.getByRole("heading", { name: model.display_name }).hover();
    await page.mouse.wheel(0, 100_000);
    await expect(bottom).toBeInViewport({ ratio: 1 });
    expect((await panePosition(row(page))).scrollTop).toBe(listY);
    await row(page, directory[81]).click();
    await expect(detail.getByRole("heading", { name: directory[81].display_name })).toBeInViewport({ ratio: 1 });
    expect(await page.evaluate(() => scrollY)).toBe(windowY);
    expect(writes).toEqual([]);
  });
}

test("model search preserves selection and drafts; real navigation still confirms discard", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const writes = await prepare(page);
  await page.goto(deepLink);
  const detail = page.getByRole("region", { name: "Operation routing" });
  const weight = detail.getByRole("spinbutton", { name: "Weight for tier 1 row 1" });
  await weight.fill("7");
  await row(page).click();
  await expect(weight).toHaveValue("7");
  await search(page).fill("DELTA CHAT 080");
  await expect(page.getByText("Models found: 2")).toBeVisible();
  await search(page).fill("MoCk-ChAt-080");
  await expect(page.getByText("Models found: 1")).toBeVisible();
  await search(page).fill("ceDar");
  await expect(page.getByText("Models found: 21")).toBeVisible();
  await expect(row(page)).toHaveCount(0);
  await expect(heading(page)).toBeAttached();
  await search(page).fill("no-such-model");
  await expect(page.getByText("No matching models")).toBeVisible();
  await page.getByRole("button", { name: "Clear search" }).click();
  await expect(search(page)).toBeFocused();
  await expect(page.getByText("Models found: 84")).toBeVisible();
  await expect(weight).toHaveValue("7");
  for (const action of [row(page, directory[81]), detail.getByRole("button", { name: /Chat Completions/ }), page.getByRole("tab", { name: "Price sync" })]) {
    await action.click();
    const confirm = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
    await confirm.getByRole("button", { name: "Cancel" }).click();
    await expect(page).toHaveURL(deepLink);
    await expect(row(page)).toHaveAttribute("aria-current", "true");
    await expect(weight).toHaveValue("7");
  }
  await row(page, directory[81]).click();
  await page.getByRole("button", { name: "Discard changes" }).click();
  await expect(detail.getByText("No operations configured yet.")).toBeVisible();
  await page.goBack();
  await expect(weight).toHaveValue("1");
  await detail.getByRole("link", { name: "Edit model" }).click();
  await page.getByRole("link", { name: "Back to client models" }).click();
  await expect(page).toHaveURL(deepLink);
  await detail.getByRole("link", { name: "Configure pricing" }).click();
  await page.getByRole("link", { name: "Back to client models" }).click();
  await expect(page).toHaveURL(deepLink);
  expect(writes).toEqual([]);
});

test("narrow list returns to the original filtered row and keyboard focus", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await prepare(page);
  await page.goto("/admin/models");
  await search(page).fill("delta");
  await row(page).scrollIntoViewIfNeeded();
  await row(page).focus();
  const listY = await page.evaluate(() => scrollY);
  expect(listY).toBeGreaterThan(500);
  const assertRestored = async () => {
    await expect(search(page)).toHaveValue("delta");
    await expect(row(page)).toBeInViewport({ ratio: 1 });
    await expect(row(page)).toBeFocused();
    expect(await page.evaluate(() => scrollY)).toBe(listY);
  };
  const back = page.getByRole("button", { name: "Back to client models" });
  await page.keyboard.press("ArrowDown");
  await expect(row(page, directory[80])).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await expect(heading(page)).toBeInViewport({ ratio: 1 });
  await expect(heading(page)).toBeFocused();
  await page.getByRole("button", { name: /Responses/ }).click();
  await expect(page.getByRole("spinbutton", { name: "Weight for tier 1 row 1" })).toBeAttached();
  await back.click();
  await assertRestored();
  await page.goForward();
  await expect(heading(page)).toBeFocused();
  await expect(page).not.toHaveURL(/rule=/);
  await page.goBack();
  await assertRestored();
  await page.keyboard.press("Enter");
  await page.getByRole("link", { name: "Configure pricing" }).click();
  await page.getByRole("link", { name: "Back to client models" }).click();
  await expect(heading(page)).toBeFocused();
  await back.click();
  await assertRestored();
  await testInfo.attach("mobile list restored", { body: await page.screenshot(), contentType: "image/png" });
});

test("short and 320px views keep the model identity and candidate fields readable", async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    localStorage.setItem("ai-gateway-console.locale", "zh-CN");
    localStorage.setItem("console-theme", "dark");
  });
  await prepare(page);
  await page.setViewportSize({ width: 320, height: 780 });
  await page.goto(deepLink);
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(heading(page)).toBeFocused();
  for (const [width, height] of [[320, 780], [844, 390]]) {
    await page.setViewportSize({ width, height });
    const capability = page.getByRole("combobox", { name: /第 1 行.*能力|能力.*第 1 行/ }).first();
    const firstCandidate = capability.locator("xpath=ancestor::li[1]");
    await capability.scrollIntoViewIfNeeded();
    expect(await firstCandidate.evaluate((listItem) => listItem.scrollWidth <= listItem.clientWidth)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await firstCandidate.getByRole("spinbutton").fill("6");
    await expect(firstCandidate.getByRole("spinbutton")).toHaveValue("6");
    await testInfo.attach(`candidate at ${width}×${height}`, { body: await page.screenshot(), contentType: "image/png" });
  }
});

test("loading, errors, empty data and a stale model ID are distinct", async ({ page }) => {
  await prepare(page);
  const responses = [null, [], directory];
  let unblock = () => {};
  const waiting = new Promise<void>((resolve) => { unblock = resolve; });
  await page.route("**/console/v1/models", async (route) => {
    const json = responses.shift();
    if (json === null) {
      await waiting;
      return route.fulfill({ status: 502, json: { error: "mock_load_failed" } });
    }
    return route.fulfill({ json });
  });
  await page.goto("/admin/models");
  await expect(page.locator('[data-slot="skeleton"]').first()).toBeVisible();
  await expect(page.getByText("No client models yet")).toHaveCount(0);
  unblock();
  await expect(page.getByText("Request failed", { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByText("No client models yet")).toBeVisible();
  await expect(page.getByRole("link", { name: "New client model" })).toBeVisible();
  await page.goto("/admin/models?model=stale");
  await expect(page.getByText("This client model is no longer available.")).toBeVisible();
  await page.getByRole("button", { name: "Back to client models" }).click();
  await expect(search(page)).toBeFocused();
  await expect(page.getByText("Models found: 84")).toBeVisible();
});

test("rule conflicts send the original ETag; a failed directory refresh keeps cached content", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.clock.setFixedTime(new Date("2026-09-01T12:00:00Z"));
  await prepare(page);
  let modelRefreshFails = false;
  await page.route("**/console/v1/models", (route) => modelRefreshFails
    ? route.fulfill({ status: 502, json: { error: "mock_refresh_failed" } })
    : route.fallback());
  let conflict = false;
  await page.route(`**/console/v1/routing/operation-rules/${operations[0].id}`, (route) => {
    if (route.request().method() === "PUT") {
      expect(route.request().headers()["if-match"]).toBe(`"${operations[0].updated_at}"`);
      expect(route.request().postDataJSON().routing_tiers[0].candidates[0].weight).toBe(7);
      conflict = true;
      return route.fulfill({ status: 409, json: { error: "conflict" } });
    }
    if (!conflict) return route.fallback();
    const newer = structuredClone(operations[0]);
    newer.updated_at = "2026-08-31T12:00:00Z";
    newer.routing_tiers[0].candidates[0].weight = 4;
    return route.fulfill({ json: newer, headers: { ETag: `"${newer.updated_at}"` } });
  });
  await page.goto("/admin/models");
  await row(page).click();
  const detail = page.getByRole("region", { name: "Operation routing" });
  await detail.getByRole("button", { name: /Responses/ }).click();
  const weight = detail.getByRole("spinbutton", { name: "Weight for tier 1 row 1" });
  await weight.fill("7");
  await page.goBack();
  await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();
  await expect(page).toHaveURL(deepLink);
  await expect(weight).toHaveValue("7");
  await detail.getByRole("button", { name: "Save operation rule" }).click();
  await expect(page.getByText("This rule was changed elsewhere. Reloading.")).toBeVisible();
  await expect(weight).toHaveValue("4");
  await page.clock.setFixedTime(new Date("2026-09-01T12:01:00Z"));
  modelRefreshFails = true;
  const nav = page.getByRole("navigation", { name: "Management navigation" });
  await nav.getByRole("button", { name: "Members and access", exact: true }).click();
  await nav.getByRole("link", { name: "Users", exact: true }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Discard changes" }).click();
  await expect(page).toHaveURL(/\/admin\/users$/);
  await expect(page.getByRole("heading", { name: "Users", exact: true, level: 1 })).toBeVisible();
  await page.goBack();
  await expect(page.getByText("Console rejected the request (mock_refresh_failed).")).toBeVisible();
  await expect(heading(page)).toBeInViewport({ ratio: 1 });
  await expect(row(page)).toHaveAttribute("aria-current", "true");
  await expect(weight).toHaveValue("4");
});
