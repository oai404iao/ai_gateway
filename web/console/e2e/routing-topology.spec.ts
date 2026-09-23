import { expect, test, type Page } from "@playwright/test";
import { E2E_MODEL, mockConsoleApi } from "./mock-api";

async function prepare(page: Page) {
  await mockConsoleApi(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill("admin@example.com");
  await page
    .getByLabel("Password", { exact: true })
    .fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/account/);
}

test.describe("canonical routing topology", () => {
  test("the routing navigation exposes the canonical editors", async ({ page }) => {
    await prepare(page);

    await page.getByRole("link", { name: "Channel configuration" }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/channels$/);
    await page.getByRole("tab", { name: "Group configuration" }).click();
    await expect(
      page.getByRole("heading", { name: "Channel groups" }),
    ).toBeVisible();
    await expect(page.getByRole("tabpanel", { name: "Group configuration" })
      .getByRole("cell", { name: "Standard group", exact: true })).toBeVisible();

    await page.getByRole("tab", { name: "Logical channels" }).click();
    await expect(page.getByText("Upstream A", { exact: true })).toBeVisible();
    await page.getByRole("combobox", { name: "Channel group" }).click();
    await page.getByRole("option", { name: "Standard group" }).click();
    await expect(page).toHaveURL(/group=/);
    await expect(page.getByRole("heading", { name: "Channel capabilities" })).toHaveCount(0);

    await page.getByText("Upstream A", { exact: true }).click();
    await page.getByRole("tab", { name: "Channel capabilities" }).click();
    await expect(page.getByRole("heading", { name: "Channel capabilities" })).toBeVisible();
    await expect(page.getByRole("columnheader", { name: "Transports" })).toHaveCount(0);
    await page.getByRole("cell", { name: "Chat Completions", exact: true }).click();
    await expect(page.getByRole("button", { name: "Save capability" })).toBeVisible();
    await expect(page).toHaveURL(/\/admin\/routing\/logical-channels\/.*capability=/);
    await expect(page.getByRole("combobox", { name: "Logical channel" })).toBeDisabled();
    const capabilityWrite = page.waitForRequest((request) =>
      request.method() === "PUT" && request.url().includes("/routing/capabilities/"));
    await page.getByRole("button", { name: "Save capability" }).click();
    expect((await capabilityWrite).headers()["if-match"]).toBeTruthy();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole("button", { name: "Save capability" })).toBeVisible();
    await expect.poll(() => page.evaluate(() =>
      document.documentElement.scrollWidth <= document.documentElement.clientWidth,
    )).toBe(true);
    await page.setViewportSize({ width: 1440, height: 1000 });

    await page.getByRole("link", { name: "Model configuration" }).click();
    await page.getByRole("button", { name: new RegExp(E2E_MODEL.display_name) }).click();
    await expect(page.getByRole("region", { name: "Operation routing" }).getByText("Cache hit price")).toBeVisible();
    await page.getByRole("button", { name: /Chat Completions/ }).click();
    await expect(page.getByRole("button", { name: "Save operation rule" })).toBeVisible();
    await expect(page).toHaveURL(/\/admin\/models\?/);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole("heading", { name: "Model configuration" })).toBeVisible();
    await expect.poll(() => page.evaluate(() =>
      document.documentElement.scrollWidth <= document.documentElement.clientWidth,
    )).toBe(true);
    await expect(page.getByRole("button", { name: "Back to client models" })).toBeVisible();
    expect(await page.getByRole("region", { name: "Operation routing" })
      .evaluate((element) => element.getBoundingClientRect().top)).toBeLessThan(450);
    await page.getByRole("button", { name: "Back to client models" }).click();
    await expect(page.getByRole("button", { name: new RegExp(E2E_MODEL.display_name) })).toBeVisible();
    await expect(page.getByRole("region", { name: "Operation routing" })).not.toBeVisible();
    await page.getByRole("tab", { name: "Price sync" }).click();
    await expect(page).toHaveURL(/view=prices/);
  });

  test("an administrator edits a channel binding with its ETag", async ({ page }) => {
    await prepare(page);
    await page.goto("/admin/routing/logical-channels");
    await page.getByText("Upstream A", { exact: true }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/logical-channels\/[0-9a-f-]+\?returnTo=/);
    await expect(page.getByLabel("Name", { exact: true })).toHaveValue(
      "Upstream A",
    );
    const requestPromise = page.waitForRequest(
      (request) =>
        request.method() === "PUT" &&
        request.url().includes("/routing/logical-channels/"),
    );
    await page.getByRole("button", { name: "Save channel", exact: true }).click();
    const request = await requestPromise;
    expect(request.headers()["if-match"]).toBeTruthy();
    expect(request.postDataJSON().credential_id).toBeNull();
    expect(request.postDataJSON().sharing_only).toBe(false);
  });

  test("unified credentials and channel sharing fit the desktop viewport", async ({ page }) => {
    await prepare(page);
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.getByRole("link", { name: "Upstream credentials" }).click();
    await page.getByRole("tab", { name: "Codex", exact: true }).click();
    await expect(page.getByText("Personal Plus", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Connect account" })).toBeEnabled();
    await expect(page.getByRole("button", { name: "Back to groups" })).toHaveCount(0);
    await expect.poll(() => page.evaluate(() =>
      document.documentElement.scrollWidth <= innerWidth,
    )).toBe(true);
    await page.goto("/admin/codex-sharing/00000000-0000-0000-0000-000000000801");
    await expect(page.getByRole("combobox", { name: "Logical channel" })).toBeDisabled();
    await expect.poll(() => page.evaluate(() =>
      document.documentElement.scrollWidth <= innerWidth,
    )).toBe(true);
  });
});
