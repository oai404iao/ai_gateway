import { expect, test, type Page } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";
import {
  CHANNEL,
  CHANNEL_GROUP,
  MODEL,
  MODEL_PROTOCOL_RULE,
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
  await page.route(
    `**/console/v1/routing/model-rules/${MODEL_RULE.id}`,
    (route) =>
      route.fulfill({
        headers: { ETag: `"${MODEL_RULE.updated_at}"` },
        json: MODEL_RULE,
      }),
  );
  await page.route(
    `**/console/v1/routing/model-rules/${MODEL_RULE.id}/protocols/${MODEL_PROTOCOL_RULE.id}`,
    (route) =>
      route.fulfill({
        headers: { ETag: `"${MODEL_PROTOCOL_RULE.updated_at}"` },
        json:
          route.request().method() === "PUT"
            ? {
                id: MODEL_PROTOCOL_RULE.id,
                correlation_id: MODEL_PROTOCOL_RULE.id,
              }
            : MODEL_PROTOCOL_RULE,
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

test("model routing drills from a priced model into its protocol", async ({
  page,
}) => {
  await prepare(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/admin/routing/model-rules");
  await expect(
    page.getByRole("heading", { name: "Model Rules", exact: true }),
  ).toBeVisible();

  await page.getByText(MODEL.display_name, { exact: true }).click();
  await expect(page).toHaveURL(
    new RegExp(`/admin/routing/model-rules/${MODEL_RULE.id}`),
  );
  await page
    .getByRole("link", { name: "Open Chat Completions", exact: true })
    .click();
  await expect(page).toHaveURL(
    new RegExp(
      `/admin/routing/model-rules/${MODEL_RULE.id}/protocols/${MODEL_PROTOCOL_RULE.id}`,
    ),
  );
  await expect(
    page.getByRole("heading", {
      name: `${MODEL.display_name} · Chat Completions`,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", {
      name: `Upstream model for channel group ${CHANNEL_GROUP.name}`,
    }),
  ).toContainText(MODEL.source_model_id);
});

test("protocol saves preserve target-owned upstream models and the ETag", async ({
  page,
}) => {
  await prepare(page);
  await page.goto(
    `/admin/routing/model-rules/${MODEL_RULE.id}/protocols/${MODEL_PROTOCOL_RULE.id}`,
  );
  await page.getByLabel("Description", { exact: true }).fill("Updated route");
  const requestPromise = page.waitForRequest(
    (request) =>
      request.method() === "PUT" &&
      request.url().endsWith(
        `/model-rules/${MODEL_RULE.id}/protocols/${MODEL_PROTOCOL_RULE.id}`,
      ),
  );
  await page
    .getByRole("button", { name: "Save protocol", exact: true })
    .click();
  const request = await requestPromise;
  expect(request.headers()["if-match"]).toBe(
    `"${MODEL_PROTOCOL_RULE.updated_at}"`,
  );
  const body = request.postDataJSON();
  expect(body.routing_tiers[0].channel_groups[0].upstream_model).toBe(
    MODEL.source_model_id,
  );
  expect(body.description).toBe("Updated route");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});
