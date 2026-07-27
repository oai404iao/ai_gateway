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

  test("a user can self-register with a reusable invitation code", async ({ page }) => {
    await mockConsoleApi(page);
    await page.goto("/register");

    await page.getByLabel("Invitation code").fill("COMMUNITY-ACCESS-2026");
    await page.getByLabel("Email").fill("new-user@example.test");
    await page.getByLabel("Display name").fill("New User");
    await page.getByLabel(/^Password$/).fill("correct-horse-battery-staple");
    await page.getByLabel("Confirm password").fill("correct-horse-battery-staple");

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/console/v1/auth/register") &&
        request.method() === "POST",
    );
    await page.getByRole("button", { name: "Create account" }).click();
    const request = await requestPromise;
    expect(request.postDataJSON()).toEqual({
      invitation_code: "COMMUNITY-ACCESS-2026",
      email: "new-user@example.test",
      display_name: "New User",
      password: "correct-horse-battery-staple",
    });
    await expect(page).toHaveURL(/\/account/);
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

  test("statistics shows personal activity and operations has system load", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();

    await page.getByRole("link", { name: "Statistics" }).click();
    await expect(page).toHaveURL(/\/statistics/);
    await expect(page.getByText("Request activity", { exact: true })).toBeVisible();
    await expect(
      page.getByLabel("6 requests on Jul 27, 2026"),
    ).toBeVisible();

    await page.getByRole("link", { name: "System load" }).click();
    await expect(page).toHaveURL(/\/admin\/system-load/);
    await expect(page.getByRole("heading", { name: "System load" })).toBeVisible();
    await expect(page.getByText("Host CPU", { exact: true })).toBeVisible();
  });

  test("users can open the spend leaderboard podium", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "Spend leaderboard" }).click();

    await expect(page).toHaveURL(/\/leaderboard/);
    await expect(page.getByText("Top spenders")).toBeVisible();
    await expect(page.getByText("Ada Lovelace").first()).toBeVisible();
    await expect(page.getByText("Diego Rivera").first()).toBeVisible();
    await expect(page.getByText("Lin Qiao").first()).toBeVisible();
    await expect(page.getByText("Leaderboard", { exact: true })).toBeVisible();
    await expect(page.getByText("1,912.06 USD")).toBeVisible();
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

  test("administrators batch-adjust selected user balances", async ({ page }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "Users" }).click();

    await page.getByRole("checkbox", { name: "Select Batch User" }).check();
    await page.getByRole("button", { name: "Batch edit (1)" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("combobox").nth(3).click();
    await page.getByRole("option", { name: "Increase balance" }).click();
    await dialog.getByLabel("Balance amount").fill("5");

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/console/v1/users/batch") &&
        request.method() === "POST",
    );
    await dialog.getByRole("button", { name: "Update users" }).click();
    const request = await requestPromise;
    expect(request.postDataJSON()).toEqual({
      items: [
        {
          id: "00000000-0000-0000-0000-000000000090",
          updated_at: "2026-01-02T00:00:00.000Z",
        },
      ],
      changes: {
        balance: {
          operation: "increase",
          amount: "5",
        },
      },
    });
    await expect(page.getByText("Updated 1 users.")).toBeVisible();
  });

  test("administrators create reusable registration invitation codes", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "Registration Codes" }).click();
    await page.getByRole("button", { name: "New registration code" }).click();

    await page.getByLabel("Name").fill("Community launch");
    await page.getByLabel("Invitation code").fill("COMMUNITY-ACCESS-2026");
    await page.getByLabel("Maximum uses").fill("100");
    await page.getByLabel(/Initial balance/i).fill("25");

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/console/v1/registration-invitation-codes") &&
        request.method() === "POST",
    );
    await page.getByRole("button", { name: "Create registration code" }).click();
    const request = await requestPromise;
    expect(request.postDataJSON()).toEqual({
      name: "Community launch",
      invitation_code: "COMMUNITY-ACCESS-2026",
      max_uses: 100,
      expires_at: null,
      enabled: true,
      user_group_id: "00000000-0000-0000-0000-000000000101",
      initial_balance_amount: "25",
    });
    await expect(
      page
        .getByRole("dialog")
        .getByRole("textbox", { name: "Registration invitation code" }),
    ).toHaveValue("COMMUNITY-ACCESS-2026");
  });
});
