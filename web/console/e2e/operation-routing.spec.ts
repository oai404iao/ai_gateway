import { expect, test, type Page } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";
import {
  CHANNEL_CAPABILITY,
  LOGICAL_CHANNEL,
  MODEL,
  OPERATION_RULE,
  ROUTING_PROFILE,
  UPSTREAM_ACCESS,
} from "../src/test/fixtures";

async function prepare(page: Page) {
  await mockConsoleApi(page);
  for (const [path, body] of Object.entries({
    models: [MODEL],
    "routing/accesses": [UPSTREAM_ACCESS],
    "routing/logical-channels": [LOGICAL_CHANNEL],
    "routing/capabilities": [CHANNEL_CAPABILITY],
    "routing/profiles": [ROUTING_PROFILE],
    "routing/operation-rules": [OPERATION_RULE],
  })) {
    await page.route(`**/console/v1/${path}`, (route) => route.fulfill({ json: body }));
  }
  await page.route(`**/console/v1/routing/operation-rules/${OPERATION_RULE.id}`, (route) =>
    route.fulfill({
      headers: { ETag: `"${OPERATION_RULE.updated_at}"` },
      json: route.request().method() === "PUT" ? { id: OPERATION_RULE.id } : OPERATION_RULE,
    }),
  );
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill("admin@example.com");
  await page.getByLabel("Password", { exact: true }).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/account/);
}

test("operation routing displays its priced model and opens the operation graph", async ({ page }) => {
  await prepare(page);
  await page.goto("/admin/routing/operation-rules");
  await page.getByText(MODEL.display_name, { exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/operation-rules/${OPERATION_RULE.id}\\?returnTo=`));
  await expect(page.getByRole("button", { name: "Save operation rule" })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }))
    .toHaveValue(OPERATION_RULE.routing_tiers[0].candidates[0].upstream_model);
});

test("operation saves preserve candidate identity and use the operation ETag", async ({ page }) => {
  await prepare(page);
  await page.goto(`/admin/routing/operation-rules/${OPERATION_RULE.id}`);
  await page.getByRole("spinbutton", { name: "Weight for tier 1 row 1" }).fill("7");
  const pending = page.waitForRequest((request) =>
    request.method() === "PUT" && request.url().endsWith(`/operation-rules/${OPERATION_RULE.id}`));
  await page.getByRole("button", { name: "Save operation rule" }).click();
  const request = await pending;
  expect(request.headers()["if-match"]).toBe(`"${OPERATION_RULE.updated_at}"`);
  expect(request.postDataJSON().routing_tiers[0].candidates).toEqual([
    { ...OPERATION_RULE.routing_tiers[0].candidates[0], weight: 7 },
  ]);
});

test("capability candidates can be searched, replaced and saved on mobile", async ({ page }) => {
  await prepare(page);
  const backup = { ...LOGICAL_CHANNEL, id: "00000000-0000-0000-0000-000000000199", name: "Backup" };
  const capability = {
    ...CHANNEL_CAPABILITY,
    id: "00000000-0000-0000-0000-000000000198",
    channel_id: backup.id,
    settings: { ...CHANNEL_CAPABILITY.settings, available_models: ["wire-backup"] },
  };
  await page.route("**/console/v1/routing/logical-channels", (route) =>
    route.fulfill({ json: [LOGICAL_CHANNEL, backup] }));
  await page.route("**/console/v1/routing/capabilities", (route) =>
    route.fulfill({ json: [CHANNEL_CAPABILITY, capability] }));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`/admin/routing/operation-rules/${OPERATION_RULE.id}`);
  await page.getByRole("button", { name: "Add record", exact: true }).click();
  await page.getByRole("combobox", { name: "Capability for tier 1 row 2" }).fill("Backup");
  await page.getByRole("option", { name: /Backup/ }).click();
  await page.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }).fill("wire-back");
  await page.getByRole("option", { name: "wire-backup", exact: true }).click();
  await page.getByRole("spinbutton", { name: "Weight for tier 1 row 2" }).fill("7");
  await page.getByRole("button", { name: "Remove tier 1 row 1" }).click();
  const pending = page.waitForRequest((request) =>
    request.method() === "PUT" && request.url().endsWith(`/operation-rules/${OPERATION_RULE.id}`));
  await page.getByRole("button", { name: "Save operation rule" }).click();
  expect((await pending).postDataJSON().routing_tiers[0].candidates).toEqual([
    { capability_id: capability.id, upstream_model: "wire-backup", weight: 7 },
  ]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
