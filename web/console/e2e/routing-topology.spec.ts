import { expect, test, type Page } from "@playwright/test";
import { E2E_CAPABILITY_ETAG, E2E_CHANNEL_ETAG, E2E_MODEL, mockConsoleApi } from "./mock-api";

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
  test("the cross-channel catalog action stays usable on narrow and short screens", async ({ page }) => {
    await prepare(page);
    await page.goto("/admin/routing/channels");
    await page.getByRole("link", { name: "All channel capabilities" }).click();
    await page.getByRole("checkbox", { name: /Select Upstream A/ }).check();
    const trigger = page.getByRole("button", { name: "Append upstream model" });
    await trigger.focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog", { name: "Append upstream model" });
    await expect(dialog).toBeVisible();
    await dialog.getByLabel("Upstream model", { exact: true }).fill("gpt-5");
    await expect(dialog.getByText("Already listed")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Apply changes" })).toBeDisabled();
    await dialog.getByLabel("Upstream model", { exact: true }).fill("new-wire");
    for (const [width, height] of [[1440, 900], [390, 844], [320, 640], [640, 320]]) {
      await page.setViewportSize({ width, height });
      const confirm = dialog.getByRole("button", { name: "Apply changes" });
      await dialog.getByRole("button", { name: "Cancel" }).focus();
      await page.keyboard.press("Tab");
      await expect(confirm).toBeFocused();
      await expect(confirm, `${width}×${height}`).toBeInViewport();
      await expect.poll(() => dialog.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    }
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    await page.getByRole("link", { name: "Back to channels" }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/channels$/);
  });

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
    await page.getByRole("link", { name: "Choose a capability to configure body and header transforms." }).click();
    await expect(page.getByRole("heading", { name: "Channel capabilities" })).toBeVisible();
    await expect(page.getByRole("columnheader", { name: "Transports" })).toHaveCount(0);
    await page.getByRole("cell", { name: "Chat Completions", exact: true }).click();
    await expect(page.getByRole("button", { name: "Save capability" })).toBeVisible();
    await expect(page).toHaveURL(/\/admin\/routing\/logical-channels\/.*capability=/);
    await expect(page.getByRole("combobox", { name: "Logical channel" })).toBeDisabled();
    await page.getByText("Body and header transforms", { exact: true }).click();
    await page.getByRole("button", { name: "Add request header rule" }).click();
    await page.getByLabel("Header name", { exact: true }).fill("x-test");
    await page.getByLabel("Header value", { exact: true }).fill("browser");
    const capabilityWrite = page.waitForRequest((request) =>
      request.method() === "PUT" && request.url().includes("/routing/capabilities/"));
    await page.getByRole("button", { name: "Save capability" }).click();
    const write = await capabilityWrite;
    expect(write.headers()["if-match"]).toBe(E2E_CAPABILITY_ETAG);
    expect(write.postDataJSON().override_document).toMatchObject({
      api_format: "open_ai_chat_completions", request_headers: { set: { "x-test": "browser" } },
    });
    await expect(page.getByText("Capability saved", { exact: true })).toBeVisible();
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
    expect(request.headers()["if-match"]).toBe(E2E_CHANNEL_ETAG);
    expect(request.postDataJSON().credential_id).toBeNull();
    await expect(page.getByText("Channel saved", { exact: true })).toBeVisible();
  });

  test("unified credentials fit the desktop viewport", async ({ page }) => {
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
  });
});
