import { expect, test, type Page } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";

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

    await page.getByRole("link", { name: "Routing groups" }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/groups$/);
    await expect(
      page.getByRole("heading", { name: "Routing groups" }),
    ).toBeVisible();
    await expect(page.getByText("Standard group")).toBeVisible();

    await page.getByRole("link", { name: "Logical channels" }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/logical-channels$/);
    await expect(page.getByText("Upstream A", { exact: true })).toBeVisible();

    await page.getByRole("link", { name: "Channel capabilities" }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/capabilities$/);
    await expect(page.getByText("HTTP JSON")).toBeVisible();

    await page.getByRole("link", { name: "Operation rules" }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/operation-rules$/);
    await expect(page.getByText("Chat Completions", { exact: true })).toBeVisible();
  });

  test("an administrator edits a channel binding with its ETag", async ({ page }) => {
    await prepare(page);
    await page.goto("/admin/routing/logical-channels");
    await page.getByText("Upstream A", { exact: true }).click();
    await expect(page).toHaveURL(/\/admin\/routing\/logical-channels\/[0-9a-f-]+$/);
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
  });
});
