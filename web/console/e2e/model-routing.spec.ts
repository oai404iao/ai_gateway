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
      name: "Upstream model for tier 1 row 1",
    }),
  ).toHaveValue(MODEL.source_model_id);
});

test("protocol saves preserve candidate-owned upstream models and the ETag", async ({
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
      request
        .url()
        .endsWith(
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
  expect(body.routing_tiers[0].candidates[0].upstream_model).toBe(
    MODEL.source_model_id,
  );
  expect(body.description).toBe("Updated route");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});

test("route records can be added, searched, edited and removed on a narrow viewport", async ({
  page,
}) => {
  await prepare(page);
  const secondChannel = {
    ...CHANNEL,
    id: "00000000-0000-0000-0000-000000000199",
    name: "Backup",
    available_models: ["wire-backup"],
  };
  await page.route("**/console/v1/routing/channels", (route) =>
    route.fulfill({ json: [CHANNEL, secondChannel] }),
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(
    `/admin/routing/model-rules/${MODEL_RULE.id}/protocols/${MODEL_PROTOCOL_RULE.id}`,
  );
  await page.getByRole("button", { name: "Add record", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }),
  ).toBeDisabled();
  await page
    .getByRole("combobox", { name: "Channel for tier 1 row 2", exact: true })
    .fill("Back");
  await page.getByRole("option", { name: /Backup/ }).click();
  await page
    .getByRole("combobox", { name: "Upstream model for tier 1 row 2" })
    .fill("wire-back");
  await page.getByRole("option", { name: "wire-backup", exact: true }).click();
  await page
    .getByRole("spinbutton", { name: "Weight for tier 1 row 2" })
    .fill("7");
  await page
    .getByRole("button", { name: "Remove tier 1 row 1", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", {
      name: "Channel for tier 1 row 1",
      exact: true,
    }),
  ).toHaveValue("Backup");
  await expect(
    page.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }),
  ).toHaveValue("wire-backup");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  const requestPromise = page.waitForRequest(
    (request) =>
      request.method() === "PUT" && request.url().includes("/protocols/"),
  );
  await page
    .getByRole("button", { name: "Save protocol", exact: true })
    .click();
  expect(
    (await requestPromise).postDataJSON().routing_tiers[0].candidates,
  ).toEqual([
    {
      channel_id: secondChannel.id,
      upstream_model: "wire-backup",
      weight: 7,
    },
  ]);
});
