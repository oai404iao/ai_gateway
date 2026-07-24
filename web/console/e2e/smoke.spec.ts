import { expect, test } from "@playwright/test";
import { E2E_API_KEY_SECRET, mockConsoleApi } from "./mock-api";

test.describe("Console SPA smoke", () => {
  test("login page renders and a successful login reaches the account shell", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");

    // The login form is visible.
    await expect(page.getByLabel(/email/i)).toBeVisible();
    await expect(page.getByLabel(/^password$/i)).toBeVisible();

    // Submit credentials; the SPA stores the access token and redirects.
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();

    // Profile is no longer duplicated in the sidebar. It remains available
    // from the user menu in the top-right corner.
    await expect(page.getByRole("link", { name: "Profile" })).toHaveCount(0);
    await page.getByRole("button", { name: /Initial Admin/ }).click();
    await expect(page.getByRole("menuitem", { name: "Profile" })).toBeVisible();
    await expect(page).toHaveURL(/\/account/);
  });

  test("an unknown deep link behind auth redirects to login when unauthenticated", async ({
    page,
  }) => {
    // No API mock for refresh -> the SPA treats the session as anonymous.
    await page.goto("/admin/users");
    await expect(page).toHaveURL(/\/login/);
  });

  test("the login UI switches to Simplified Chinese and retains the preference", async ({
    page,
  }) => {
    await page.goto("/login");

    await page.getByRole("button", { name: "Language" }).click();
    await page.getByRole("menuitemradio", { name: "简体中文" }).click();
    await expect(page.getByRole("button", { name: "登录" })).toBeVisible();
    await expect(page.getByLabel("邮箱")).toBeVisible();

    await page.reload();
    await expect(page.getByRole("button", { name: "登录" })).toBeVisible();
  });

  test("API keys stay masked until explicitly revealed", async ({ page }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "API Keys" }).click();

    await expect(page.getByRole("textbox", { name: "API host" })).toHaveValue(
      "https://api.e2e.example.test/v1",
    );
    const keyValue = page.getByLabel("API key value");
    await expect(keyValue).toHaveValue(`sk-${"•".repeat(24)}`);
    await page.getByRole("button", { name: "Show full API key" }).click();
    await expect(keyValue).toHaveValue(E2E_API_KEY_SECRET);
  });

  test("all users can open the Channel status page", async ({ page }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "Channel status" }).click();

    await expect(page).toHaveURL(/\/channel-status/);
    await expect(page.getByRole("heading", { name: "Channel status" })).toBeVisible();
    await expect(page.getByText("Model overview")).toBeVisible();
  });

  test("users choose API key targets and per-key limits", async ({ page }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "API Keys" }).click();
    await page.getByRole("button", { name: "New API key" }).click();

    await page.getByLabel(/^name$/i).fill("browser key");
    await page
      .getByRole("checkbox", { name: "chat-primary (Chat Completions)" })
      .check();
    await page.getByLabel("Requests / minute").fill("45");
    await page.getByLabel("Max concurrent requests").fill("3");
    await page.getByLabel("Quota limit amount").fill("12.50");

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/console/v1/me/api-keys") &&
        request.method() === "POST",
    );
    await page.getByRole("button", { name: "Create key" }).click();
    const request = await requestPromise;
    expect(request.postDataJSON()).toEqual({
      name: "browser key",
      expires_at: null,
      allowed_group_ids: ["00000000-0000-0000-0000-000000000021"],
      allowed_channel_ids: [],
      requests_per_minute: 45,
      max_concurrent_requests: 3,
      quota_limit_amount: "12.50",
    });
    await expect(page.getByText("API key created")).toBeVisible();
  });
});
