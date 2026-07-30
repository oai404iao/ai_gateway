import { expect, test } from "@playwright/test";
import {
  E2E_API_KEY_SECRET,
  E2E_CODEX_CREDENTIAL,
  E2E_CODEX_CREDENTIAL_ID,
  E2E_CODEX_GROUP_ID,
  mockConsoleApi,
} from "./mock-api";

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

  test("users can identify and revoke a specific login session", async ({ page }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "Login sessions" }).click();

    await expect(page).toHaveURL(/\/account\/sessions/);
    await expect(page.getByText("Safari 18 · macOS")).toBeVisible();
    await expect(page.getByText("Current device")).toBeVisible();
    await expect(page.getByText("Firefox 128 · Windows")).toBeVisible();

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith(
          "/console/v1/me/sessions/00000000-0000-0000-0000-0000000000e2",
        ) && request.method() === "DELETE",
    );
    await page
      .getByRole("button", { name: "Sign out Firefox 128 · Windows" })
      .click();
    await page
      .getByRole("alertdialog")
      .getByRole("button", { name: "Sign out", exact: true })
      .click();
    await requestPromise;
    await expect(page.getByText("Device signed out")).toBeVisible();

    await page.getByRole("button", { name: "Show history" }).click();
    await expect(page.getByText("Revoked session", { exact: true })).toBeVisible();
  });

  test("users can enable Responses WebSocket in personal settings", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await page.getByRole("link", { name: "Personal settings" }).click();

    await expect(page).toHaveURL(/\/account\/settings/);
    const toggle = page.getByRole("switch", {
      name: "Enable Responses WebSocket",
    });
    await expect(toggle).not.toBeChecked();
    await toggle.click();

    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/console/v1/me/settings") &&
        request.method() === "PUT",
    );
    await page.getByRole("button", { name: "Save personal settings" }).click();
    const request = await requestPromise;
    expect(request.postDataJSON()).toEqual({ websocket_enabled: true });
    await expect(page.getByText("Personal settings saved.")).toBeVisible();
  });

  test("administrators can test a proxy draft and inspect its egress IP", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await expect(page).toHaveURL(/\/account/);
    await page.evaluate((path) => {
      window.history.pushState({}, "", path);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, "/admin/network/proxies/new");
    await expect(page).toHaveURL(/\/admin\/network\/proxies\/new$/);

    await page.getByLabel("Proxy URL").fill("http://127.0.0.1:8080");
    await page.getByLabel("Username").fill("proxy-user");
    await page.getByLabel("Password").fill("proxy-password");
    const requestPromise = page.waitForRequest(
      (request) =>
        request.url().endsWith("/console/v1/network/proxies/test") &&
        request.method() === "POST",
    );
    await page.getByRole("button", { name: "Test proxy" }).click();
    const request = await requestPromise;

    expect(request.postDataJSON()).toEqual({
      proxy_url: "http://127.0.0.1:8080",
      username: "proxy-user",
      password: "proxy-password",
    });
    await expect(page.getByText("Proxy test result")).toBeVisible();
    await expect(page.getByText("203.0.113.10")).toBeVisible();
    await expect(page.getByText("E2E ISP")).toBeVisible();
  });

  test("administrators can inspect and batch-update a Codex OAuth credential", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();
    await expect(page).toHaveURL(/\/account/);
    await page.evaluate((path) => {
      window.history.pushState({}, "", path);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, `/admin/providers/codex-oauth/${E2E_CODEX_GROUP_ID}`);
    await expect(page).toHaveURL(
      new RegExp(`/admin/providers/codex-oauth/${E2E_CODEX_GROUP_ID}$`),
    );

    await expect(
      page.getByRole("heading", { name: "Codex subscriptions" }),
    ).toBeVisible();
    await expect(page.getByText("Personal Plus")).toBeVisible();
    await expect(page.getByText("96% used")).toBeVisible();
    await expect(page.getByText("Draining", { exact: true })).toBeVisible();

    const editButton = page.getByRole("button", {
      name: "Edit Personal Plus",
    });
    const quotaButton = page.getByRole("button", {
      name: "Refresh quota for Personal Plus",
    });
    const tokenButton = page.getByRole("button", {
      name: "Refresh token for Personal Plus",
    });
    const deleteButton = page.getByRole("button", {
      name: "Delete Personal Plus",
    });
    for (const [button, label] of [
      [editButton, "Edit Personal Plus"],
      [quotaButton, "Refresh quota for Personal Plus"],
      [tokenButton, "Refresh token for Personal Plus"],
      [deleteButton, "Delete Personal Plus"],
    ] as const) {
      await button.hover();
      await expect(page.getByText(label, { exact: true })).toBeVisible();
    }

    const [editBox, quotaBox, tokenBox, deleteBox] = await Promise.all([
      editButton.boundingBox(),
      quotaButton.boundingBox(),
      tokenButton.boundingBox(),
      deleteButton.boundingBox(),
    ]);
    expect(editBox).not.toBeNull();
    expect(quotaBox).not.toBeNull();
    expect(tokenBox).not.toBeNull();
    expect(deleteBox).not.toBeNull();
    expect(editBox!.y).toBeLessThan(quotaBox!.y);
    expect(Math.abs(quotaBox!.y - tokenBox!.y)).toBeLessThan(2);
    expect(Math.abs(quotaBox!.y - deleteBox!.y)).toBeLessThan(2);

    const refresh = page.waitForRequest(
      (request) =>
        request.url().endsWith(
          `/console/v1/providers/codex-oauth/credentials/${E2E_CODEX_CREDENTIAL_ID}/quota/refresh`,
        ) && request.method() === "POST",
    );
    await quotaButton.click();
    await refresh;
    await expect(page.getByText("Quota refreshed.")).toBeVisible();

    await page
      .getByRole("checkbox", { name: "Select Personal Plus" })
      .click();
    const batch = page.waitForRequest(
      (request) =>
        request.url().endsWith(
          `/console/v1/providers/codex-oauth/channel-groups/${E2E_CODEX_GROUP_ID}/credentials/batch`,
        ) && request.method() === "POST",
    );
    await page.getByRole("button", { name: "Disable" }).click();
    expect((await batch).postDataJSON()).toEqual({
      items: [
        {
          id: E2E_CODEX_CREDENTIAL_ID,
          updated_at: E2E_CODEX_CREDENTIAL.updated_at,
        },
      ],
      operation: "disable",
    });
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

  test("personal and system request logs expose the intended fields", async ({
    page,
  }) => {
    await mockConsoleApi(page);
    await page.goto("/login");
    await page.getByLabel(/email/i).fill("admin@example.com");
    await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
    await page.getByRole("button", { name: /sign in/i }).click();

    await page.locator('a[href="/usage/request-logs"]').click();
    await expect(page).toHaveURL(/\/usage\/request-logs/);
    await expect(
      page.getByRole("columnheader", { name: "Started" }),
    ).toBeVisible();
    expect(await page.getByRole("columnheader").allTextContents()).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
    ]);
    await expect(page.getByText("upstream-a", { exact: true })).toHaveCount(0);
    await expect(page.getByLabel("Reasoning effort: High")).toBeVisible();
    await expect(page.getByLabel("Fast mode")).toBeVisible();
    await expect(page.getByLabel("TTFT: 120 ms")).toBeVisible();
    await expect(page.getByLabel("Total duration: 1 s")).toBeVisible();
    await expect(page.getByLabel("TPS: 4.5 tok/s")).toBeVisible();

    const waitForPersonalLogQuery = () =>
      page.waitForRequest((request) => {
        const url = new URL(request.url());
        return (
          url.pathname === "/console/v1/me/request-logs" &&
          request.method() === "GET"
        );
      });
    const applyFilters = page.getByRole("button", { name: "Apply" });
    let repeatedQuery = waitForPersonalLogQuery();
    await applyFilters.click();
    await repeatedQuery;
    repeatedQuery = waitForPersonalLogQuery();
    await applyFilters.click();
    await repeatedQuery;

    await page.getByRole("cell", { name: "gateway-e2e-model" }).click();
    const personalDetail = page.getByRole("dialog");
    await expect(personalDetail).toBeVisible();
    await expect(
      personalDetail.locator("dt").filter({ hasText: "Completed" }),
    ).toBeVisible();
    expect(await personalDetail.locator("dt").allTextContents()).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "HTTP",
      "Error code",
      "Error message",
      "Completed",
    ]);
    await expect(page.getByText("upstream-a", { exact: true })).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(personalDetail).toBeHidden();

    await page.locator('a[href="/admin/request-logs"]').click();
    await expect(page).toHaveURL(/\/admin\/request-logs/);
    await expect(
      page.getByRole("columnheader", { name: "User", exact: true }),
    ).toBeVisible();
    expect(await page.getByRole("columnheader").allTextContents()).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Channel",
      "User",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
    ]);
    await expect(page.getByRole("cell", { name: "upstream-a" })).toBeVisible();
    await expect(page.getByRole("cell", { name: "Batch User" })).toBeVisible();
    await page.getByRole("cell", { name: "gateway-e2e-model" }).click();
    const systemDetail = page.getByRole("dialog");
    await expect(systemDetail).toBeVisible();
    await expect(
      systemDetail.locator("dt").filter({ hasText: "Completed" }),
    ).toBeVisible();
    expect(await systemDetail.locator("dt").allTextContents()).toEqual([
      "Started",
      "Model",
      "Protocol",
      "Channel group",
      "Channel",
      "User",
      "Outcome",
      "Tokens",
      "Cost",
      "Duration",
      "HTTP",
      "Error code",
      "Error message",
      "Completed",
    ]);
    await expect(
      systemDetail.getByText("upstream-a", { exact: true }),
    ).toBeVisible();
    await expect(
      systemDetail.getByText("Batch User", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("00000000-0000-0000-0000-000000000022", {
        exact: true,
      }),
    ).toHaveCount(0);
    await expect(
      page.getByText("00000000-0000-0000-0000-000000000090", {
        exact: true,
      }),
    ).toHaveCount(0);
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
