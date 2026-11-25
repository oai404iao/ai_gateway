import { expect, test, type Page } from "@playwright/test";
import type { ModelOnboardingInput } from "../src/api/types";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL, MODEL, OPERATION_RULE, ROUTING_GROUP, ROUTING_PROFILE, UPSTREAM_ACCESS } from "../src/test/fixtures";
import { mockConsoleApi } from "./mock-api";

const name = "demo-chat-v2";
const model = { ...MODEL, display_name: "Demo Chat v2", source_model_id: name };
const old = "2026-04-01T00:00:00Z", latest = "2026-04-02T00:00:00Z";
const id = (n: number) => `00000000-0000-0000-0000-${String(n).padStart(12, "0")}`;
const accesses = ["北辰", "南岭"].map((name, i) => ({ ...UPSTREAM_ACCESS, name, id: id(3200 + i) }));
const channels = accesses.map((access, i) => ({ ...LOGICAL_CHANNEL, name: i ? "B" : "A", id: id(3300 + i), access_id: access.id, credential_id: null }));
const caps = (["chat_completion", "responses"] as const).flatMap((operation, op) => channels.map((channel, provider) => ({
  ...CHANNEL_CAPABILITY, id: id(3400 + op * 2 + provider), channel_id: channel.id, settings: {
    ...CHANNEL_CAPABILITY.settings, operation, available_models: op && provider ? ["alternate-wire"] : [name],
  },
})));

async function prepare(page: Page, existing: boolean) {
  await mockConsoleApi(page);
  let version = old, saved = false;
  const posts: ModelOnboardingInput[] = [];
  const writes: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/console/v1/") && !path.startsWith("/console/v1/auth/") && request.method() !== "GET") {
      writes.push(`${request.method()} ${path}`);
    }
  });
  const rule = () => ({ ...OPERATION_RULE, operation: "chat_completion", updated_at: version,
    routing_tiers: [{ priority: 0, selection_strategy: "weighted_round_robin", candidates: [
      { capability_id: caps[0].id, upstream_model: name, weight: 7 },
      ...(version === old ? [] : [{ capability_id: caps[1].id, upstream_model: name, weight: 3 }]),
    ] }],
  });
  const reads: Record<string, () => unknown> = {
    models: () => existing || saved ? [{ ...model, updated_at: version }] : [],
    "routing/profiles": () => existing ? [{ ...ROUTING_PROFILE, client_model: name }] : [],
    "routing/operation-rules": () => existing ? [rule()] : [],
    "routing/groups": () => [ROUTING_GROUP], "routing/accesses": () => accesses,
    "routing/logical-channels": () => channels, "routing/capabilities": () => caps.map(c => ({ ...c, updated_at: version })),
  };
  for (const [path, read] of Object.entries(reads)) {
    await page.route(`**/console/v1/${path}`, (r) => r.fulfill({ json: read() }));
  }
  await page.route("**/console/v1/models/onboarding", (r) => {
    posts.push(r.request().postDataJSON());
    if (existing && version === old) {
      version = latest;
      return r.fulfill({ status: 409, json: { error: "conflict" } });
    }
    saved = true;
    return r.fulfill({ json: { model_id: model.id } });
  });
  await page.goto("/login");
  await page.getByLabel(/^(Email|邮箱)$/).fill("admin@example.com");
  await page.getByLabel(/^(Password|密码)$/).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: /^(Sign in|登录)$/ }).click();
  await expect(page).toHaveURL("/");
  return { posts, writes };
}

test.describe("联合上架", () => {
  test.use({ locale: "zh-CN" });
  for (const [width, height] of [[1440, 900], [390, 844]]) {
    test(`review two providers and two operations before one write at ${width}×${height}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height });
      const { posts, writes } = await prepare(page, false);
      await page.goto("/admin/models");
      await page.getByRole("link", { name: "上架模型" }).click();
      await page.getByLabel("客户端模型 ID").fill(name);
      await page.getByLabel(/^输入单价/).fill("0.80");
      await page.getByLabel(/^缓存输入单价/).fill("0.08");
      await page.getByLabel(/^缓存写入单价/).fill("0.90");
      await page.getByLabel(/^输出单价/).fill("2.40");
      await testInfo.attach(`确认价格 ${width}×${height}`, { body: await page.screenshot(), contentType: "image/png" });
      await page.getByRole("button", { name: "预览路由候选" }).click();
      await expect(page.getByRole("heading", { name: /2 · 路由候选/ })).toBeFocused();
      await expect(page.getByText("基础价格 · USD / 1M Token", { exact: true })).toBeVisible();
      const missing = page.getByRole("checkbox", { name: /登记并路由.*南岭.*Responses$/ });
      await expect(missing).not.toBeChecked();
      await expect(page.getByText("2 项操作 · 3 个新候选 · 0 项模型登记")).toBeVisible();
      await missing.check();
      await expect(page.getByText("2 项操作 · 4 个新候选 · 1 项模型登记")).toBeVisible();
      expect(writes).toEqual([]);
      const apply = page.getByRole("button", { name: "确认上架" });
      await expect(apply).toBeInViewport({ ratio: 1 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      const screenshot = testInfo.outputPath("model-onboarding-review.png");
      await page.screenshot({ path: screenshot });
      await testInfo.attach(`核对候选 ${width}×${height}`, { path: screenshot, contentType: "image/png" });
      await apply.click();
      await expect(page).toHaveURL(new RegExp(`/admin/models\\?model=${model.id}$`));
      expect(writes).toEqual(["POST /console/v1/models/onboarding"]);
      expect(posts[0].model).toMatchObject({ create: { source_model_id: name, input_unit_price: "0.80", cached_input_unit_price: "0.08",
        cache_write_unit_price: "0.90", output_unit_price: "2.40", price_unit_tokens: 1_000_000 } });
      for (const operation of ["chat_completion", "responses"] as const) {
        const candidates = caps.filter(c => c.settings.operation === operation).map(c => ({ capability_id: c.id, updated_at: old,
          upstream_model: name, priority: 0, register_model: !c.settings.available_models.includes(name) }));
        expect(posts[0].operations.find(op => op.operation === operation)).toEqual({ operation, rule_updated_at: null, candidates: expect.arrayContaining(candidates) });
      }
      expect(posts[0].operations.flatMap(op => op.candidates)).toHaveLength(4);
    });
  }
});

test("existing model uses only advertised candidates and requires a fresh review after conflict", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const { posts, writes } = await prepare(page, true);
  await page.goto(`/admin/models?model=${model.id}`);
  await page.getByRole("link", { name: "Bulk configure routes" }).click();
  const apply = page.getByRole("button", { name: "Apply route additions" });
  await expect(page.getByText("Already in this tier · weight 7")).toBeVisible();
  await expect(page.getByRole("checkbox", { name: /Register and route/ })).toHaveCount(0);
  const missing = page.getByRole("checkbox", { name: /南岭.*Responses$/ });
  await expect(missing).toBeDisabled();
  await expect(page.getByText("2 operations · 2 candidates · 0 catalog additions")).toBeVisible();
  expect(writes).toEqual([]);
  await apply.click();
  await expect(page.getByText("Configuration changed. Nothing was applied.")).toBeVisible();
  await expect(apply).toBeDisabled();
  expect(posts[0].model).toEqual({ existing: { id: model.id, updated_at: old } });
  expect(posts[0].operations.find(op => op.operation === "chat_completion")).toMatchObject({ rule_updated_at: old });
  await page.getByRole("button", { name: "Reload preview" }).click();
  await expect(page.getByText("Already in this tier · weight 3")).toBeVisible();
  await expect(page.getByText(/1 operations? · 1 candidates? · 0 catalog additions/)).toBeVisible();
  expect(writes).toHaveLength(1);
  await page.getByRole("combobox", { name: /Routing tier for 南岭.*Chat Completions$/ }).click();
  await page.getByRole("option", { name: "Priority 1 · new tier" }).click();
  await page.getByRole("checkbox", { name: /南岭.*Chat Completions$/ }).check();
  await page.getByRole("combobox", { name: /Upstream model for 南岭.*Responses$/ }).fill("alternate-wire");
  await page.getByRole("option", { name: "alternate-wire" }).click();
  await missing.check();
  await page.getByRole("link", { name: "Back to client models" }).click();
  await page.getByRole("alertdialog", { name: "Discard unsaved changes?" }).getByRole("button", { name: "Cancel" }).click();
  await expect(missing).toBeChecked();
  await apply.click();
  await expect(page).toHaveURL(new RegExp(`/admin/models\\?model=${model.id}$`));
  expect(writes).toEqual(Array(2).fill("POST /console/v1/models/onboarding"));
  expect(posts[1].model).toEqual({ existing: { id: model.id, updated_at: latest } });
  expect(posts[1].operations).toHaveLength(2);
  expect(posts[1].operations.find(op => op.operation === "chat_completion")).toEqual({ operation: "chat_completion", rule_updated_at: latest,
    candidates: [{ capability_id: caps[1].id, upstream_model: name, register_model: false, priority: 1, updated_at: latest }] });
  const responses = posts[1].operations.find(op => op.operation === "responses");
  expect(responses).toEqual({ operation: "responses", rule_updated_at: null, candidates: expect.arrayContaining([
    { capability_id: caps[2].id, upstream_model: name, register_model: false, priority: 0, updated_at: latest },
    { capability_id: caps[3].id, upstream_model: "alternate-wire", register_model: false, priority: 0, updated_at: latest },
  ]) });
  expect(responses?.candidates).toHaveLength(2);
});

test("a lost response never reports success or retries automatically", async ({ page }) => {
  const { writes } = await prepare(page, true);
  await page.route("**/console/v1/models/onboarding", r => r.abort("connectionreset"));
  await page.goto(`/admin/models/${model.id}/onboarding`);
  const apply = page.getByRole("button", { name: "Apply route additions" });
  await apply.click();
  await expect(page.getByText("Result unknown. Reload the preview before trying again.")).toBeVisible();
  await expect(apply).toBeDisabled();
  await page.getByRole("button", { name: "Reload preview" }).click();
  await expect(apply).toBeEnabled();
  expect(writes).toEqual(["POST /console/v1/models/onboarding"]);
});
