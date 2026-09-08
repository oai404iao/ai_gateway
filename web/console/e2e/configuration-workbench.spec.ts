import { expect, test, type Page } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";
import {
  CHANNEL,
  CHANNEL_GROUP,
  MODEL,
  MODEL_RULE,
} from "../src/test/fixtures";

async function prepare(page: Page) {
  await mockConsoleApi(page);
  const lists = {
    models: [MODEL],
    "routing/channel-groups": [CHANNEL_GROUP],
    "routing/channels": [CHANNEL],
    "routing/model-rules": [MODEL_RULE],
  };
  for (const [path, body] of Object.entries(lists)) {
    await page.route(`**/console/v1/${path}`, (route) =>
      route.fulfill({ json: body }),
    );
  }
  await page.route(`**/console/v1/models/${MODEL.id}`, (route) =>
    route.fulfill({
      headers: { ETag: `"${MODEL.updated_at}"` },
      json:
        route.request().method() === "PUT"
          ? { id: MODEL.id, correlation_id: MODEL.id }
          : MODEL,
    }),
  );
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill("admin@example.com");
  await page
    .getByLabel("Password", { exact: true })
    .fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/account/);
}

test("workbench shows a split request path and traverses route, model and supply", async ({
  page,
}) => {
  await prepare(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page
    .getByRole("link", { name: "Model configuration", exact: true })
    .click();
  const directory = page.getByRole("region", {
    name: "Configuration directory",
  });
  const inspector = page.getByRole("region", {
    name: "Configuration inspector",
  });
  await expect(
    inspector.getByRole("heading", { name: MODEL_RULE.client_model }),
  ).toBeVisible();
  const left = await directory.boundingBox();
  const right = await inspector.boundingBox();
  expect(left!.x + left!.width).toBeLessThan(right!.x);
  await inspector
    .getByRole("link", { name: new RegExp(MODEL.display_name) })
    .click();
  await expect(page).toHaveURL(
    new RegExp(`/admin/models\\?selected=${MODEL.id}`),
  );
  await expect(
    inspector.getByRole("heading", { name: "Pricing" }),
  ).toBeVisible();
  await inspector
    .getByRole("link", { name: new RegExp(CHANNEL_GROUP.name) })
    .click();
  await expect(page).toHaveURL(/\/admin\/routing\/channels\?selected=/);
  await expect(
    inspector.getByRole("heading", { name: "Routes using this group" }),
  ).toBeVisible();
  await page.goBack();
  await expect(
    inspector.getByRole("heading", { name: MODEL.display_name }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/workbench-models-desktop.png",
    fullPage: true,
  });
});

test("dirty drafts survive browser Back and successful saves restore directory context", async ({
  page,
}) => {
  await prepare(page);
  const returnTo = `/admin/models?q=mini&facet=OpenAI&selected=${MODEL.id}`;
  await page.goto(returnTo);
  await page.getByRole("link", { name: "Edit model", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Save upstream model", exact: true }),
  ).toBeInViewport();
  await page.getByLabel("Display name", { exact: true }).fill("Updated mini");
  await page.evaluate(() => window.history.back());
  const dialog = page.getByRole("alertdialog", {
    name: "Discard unsaved changes?",
  });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByLabel("Display name", { exact: true })).toHaveValue(
    "Updated mini",
  );
  const request = page.waitForRequest(
    (request) =>
      request.method() === "PUT" &&
      request.url().endsWith(`/models/${MODEL.id}`),
  );
  await page
    .getByRole("button", { name: "Save upstream model", exact: true })
    .click();
  expect((await request).headers()["if-match"]).toBe(`"${MODEL.updated_at}"`);
  await expect(page).toHaveURL(
    new RegExp(`${returnTo.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
  );
  await expect(
    page.getByRole("searchbox", { name: "Search configuration" }),
  ).toHaveValue("mini");
});

test("mobile uses directory then inspector, with no document overflow", async ({
  page,
}) => {
  await prepare(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/admin/models");
  const directory = page.getByRole("region", {
    name: "Configuration directory",
  });
  const inspector = page.getByRole("region", {
    name: "Configuration inspector",
  });
  await expect(directory).toBeVisible();
  await expect(inspector).toBeHidden();
  await directory
    .getByRole("link", { name: new RegExp(MODEL.display_name) })
    .click();
  await expect(inspector).toBeVisible();
  await expect(directory).toBeHidden();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/workbench-models-mobile.png",
    fullPage: true,
  });
  await page
    .getByRole("button", { name: "Back to directory", exact: true })
    .click();
  await expect(directory).toBeVisible();
  await expect(inspector).toBeHidden();
  await page.setViewportSize({ width: 768, height: 1024 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});

test("Chinese dark workbench remains readable and keyboard navigable", async ({
  page,
}) => {
  await prepare(page);
  await page.evaluate(() => {
    localStorage.setItem("ai-gateway-console.locale", "zh-CN");
    localStorage.setItem("console-theme", "dark");
  });
  await page.goto("/admin/routing/model-rules");
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(
    page.getByRole("heading", { name: "模型配置", exact: true }),
  ).toBeVisible();
  const result = page
    .getByRole("region", { name: "配置目录" })
    .getByRole("link", { name: new RegExp(MODEL_RULE.client_model) });
  await result.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("region", { name: "配置关系面板" }),
  ).toBeFocused();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/workbench-routes-zh.png",
    fullPage: true,
  });
});
