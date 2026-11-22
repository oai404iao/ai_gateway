import { expect, test, type Page } from "@playwright/test";
import { E2E_ROUTING_GROUP_ID, E2E_STANDARD_CHANNEL_ID, E2E_USER, mockConsoleApi } from "./mock-api";

async function login(page: Page, email = "admin@example.com") {
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill(email);
  await page.getByLabel("Password", { exact: true }).fill("synthetic-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL("/");
}

test("an untouched account page can navigate without a discard prompt", async ({ page }) => {
  await login(page);
  await page.getByRole("button", { name: "Account menu" }).click();
  await page.getByRole("menuitem", { name: "Profile", exact: true }).click();
  await expect(page.getByLabel("Display name", { exact: true })).toHaveValue("Initial Admin");
  await page.getByRole("button", { name: "Change password" }).click();
  await expect(page.getByLabel("Current password", { exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("navigation", { name: "Console navigation" }).getByRole("link", { name: "Management" }).click();
  await page.getByRole("navigation", { name: "Management navigation" }).getByRole("link", { name: "Channel configuration" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Channel configuration" })).toBeVisible();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
});

for (const admin of [false, true]) {
  test(`${admin ? "admins" : "members"} land on their own overview with role-scoped navigation`, async ({ page }) => {
    const reads = new Set<string>();
    page.on("request", req => {
      const path = new URL(req.url()).pathname;
      if (req.method() === "GET" && path.startsWith("/console/v1/")) reads.add(path.slice("/console/v1".length));
    });
    await login(page, admin ? "admin@example.com" : E2E_USER.email);
    const nav = page.getByRole("navigation", { name: "Console navigation" });
    for (const [name, href] of [
      ["Overview", "/"], ["Usage", "/statistics"], ["Request logs", "/usage/request-logs"],
      ["API keys", "/api-keys"], ["Services", "/channel-group-status"],
    ]) {
      await expect(nav.getByRole("link", { name, exact: true })).toHaveAttribute("href", href);
    }
    await expect(nav.getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");
    await expect(page.getByRole("heading", { level: 1, name: "Gateway overview" })).toBeVisible();
    const interval = page.getByRole("link", { name: "Interval logs" });
    const latestInterval = await interval.getAttribute("href");
    expect(latestInterval).toMatch(/^\/usage\/request-logs\?.*started_after=/);
    await page.getByRole("button", { name: "Previous interval" }).click();
    await expect(interval).not.toHaveAttribute("href", latestInterval!);
    const tokens = page.getByRole("button", { name: /^Total tokens/ });
    await tokens.click();
    await expect(tokens).toHaveAttribute("aria-pressed", "true");
    await page.getByRole("tab", { name: "Recent requests" }).click();
    await expect(page.getByText("gateway-e2e-model", { exact: true })).toBeVisible();
    expect(reads).toContain("/statistics/costs");
    expect(reads).toContain("/me/request-logs");
    expect(reads.has("/request-logs")).toBe(false);
    expect(reads.has("/system/statistics/costs")).toBe(false);

    const manage = nav.getByRole("link", { name: "Management" });
    const finder = page.getByRole("dialog", { name: "Find page" });
    const managementBand = page.getByRole("region", { name: "Gateway management" });
    if (admin) await expect(managementBand.getByRole("link", { name: "Global request logs" })).toBeVisible();
    else await expect(managementBand).toHaveCount(0);
    if (admin) await page.keyboard.press("Control+k");
    else await page.getByRole("button", { name: "Find page" }).click();
    await finder.getByRole("combobox").fill("System load");
    if (admin) {
      await expect(finder.getByRole("option", { name: /System load/ })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(manage).toHaveAttribute("href", "/admin/models");
      await manage.click();
      await expect(page.getByRole("navigation", { name: "Management navigation" })).toBeVisible();
    } else {
      await expect(finder.getByRole("option")).toHaveCount(0);
      await page.keyboard.press("Escape");
      await expect(finder).toBeHidden();
      await expect(manage).toHaveCount(0);
      await expect(page.locator('a[href^="/admin/"]')).toHaveCount(0);
      for (const path of ["/admin/models", "/admin/system/upstream"]) {
        await page.goto(path);
        await expect(page).toHaveURL("/");
        await expect(page.getByRole("navigation", { name: "Management navigation" })).toHaveCount(0);
      }
      await expect(nav.getByRole("link", { name: "Overview" })).toBeVisible();
      expect([...reads].filter(path => !path.startsWith("/me") && !path.startsWith("/statistics/"))).toEqual([]);
    }
  });
}

test("management groups disclose destinations and deep links restore the active settings branch", async ({ page }) => {
  await login(page);
  await page.getByRole("navigation", { name: "Console navigation" }).getByRole("link", { name: "Management" }).click();
  const nav = page.getByRole("navigation", { name: "Management navigation" });
  const group = (name: string) => nav.getByRole("button", { name, exact: true });
  const leaf = (name: string) => nav.getByRole("link", { name, exact: true });
  await expect(group("Model routing")).toHaveAttribute("aria-expanded", "true");
  await expect(leaf("Model configuration")).toHaveAttribute("aria-current", "page");
  for (const name of ["Members and access", "Operations", "System settings"]) {
    await expect(group(name)).toHaveAttribute("aria-expanded", "false");
  }
  await expect(leaf("Plugins")).toBeHidden();
  await group("System settings").press("Enter");
  await expect(leaf("Plugins")).toBeVisible();
  await expect(group("Connections and protocols")).toHaveAttribute("aria-expanded", "false");
  await expect(leaf("Upstream timeouts")).toBeHidden();
  await group("Connections and protocols").press("Space");
  await expect(leaf("Upstream timeouts")).toBeVisible();
  await expect(page).toHaveURL("/admin/models");

  await leaf("Upstream timeouts").click();
  await expect(page).toHaveURL("/admin/system/upstream");
  await page.reload();
  await expect(group("System settings")).toHaveAttribute("aria-expanded", "true");
  await expect(group("Connections and protocols")).toHaveAttribute("aria-expanded", "true");
  await expect(leaf("Upstream timeouts")).toHaveAttribute("aria-current", "page");
  const timeout = page.getByLabel("Connect timeout (seconds)");
  await timeout.fill("27");
  await group("Connections and protocols").click();
  await expect(leaf("API hosts")).toBeHidden();
  await expect(page).toHaveURL("/admin/system/upstream");
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await group("Connections and protocols").click();
  await expect(timeout).toHaveValue("27");
  await leaf("API hosts").click();
  const guard = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
  await guard.getByRole("button", { name: "Cancel" }).click();
  await expect(timeout).toHaveValue("27");
  await expect(leaf("Upstream timeouts")).toHaveAttribute("aria-current", "page");
  await leaf("API hosts").click();
  await guard.getByRole("button", { name: "Discard changes" }).click();
  await expect(page).toHaveURL("/admin/system/general");
  await expect(leaf("API hosts")).toHaveAttribute("aria-current", "page");
});

test("mobile bottom navigation provides direct core routes and searches other pages", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await login(page, E2E_USER.email);
  const nav = page.getByRole("navigation", { name: "Mobile navigation" });
  await expect(page.getByRole("navigation", { name: "Console navigation" })).toBeHidden();
  for (const [name, href] of [["Request logs", "/usage/request-logs"], ["API keys", "/api-keys"], ["Overview", "/"]]) {
    const link = nav.getByRole("link", { name, exact: true });
    await expect(link).toBeInViewport({ ratio: 1 });
    await link.click();
    await expect(page).toHaveURL(href);
    await expect(link).toHaveAttribute("aria-current", "page");
  }
  await nav.getByRole("button", { name: "More" }).click();
  const finder = page.getByRole("dialog", { name: "Find page" });
  await finder.getByRole("combobox").fill("Spend leaderboard");
  await finder.getByRole("option", { name: /Spend leaderboard/ }).click();
  await expect(page).toHaveURL("/leaderboard");
  await expect(finder).toBeHidden();
  await expect(page.getByRole("heading", { name: "Spend leaderboard", level: 1 })).toBeVisible();
});

test("browser Back, tabs and parent links share the same draft protection", async ({ page }) => {
  await login(page);
  const origin = `/admin/routing/channels?group=${E2E_ROUTING_GROUP_ID}`;
  await page.goto(origin);
  const channel = page.getByRole("link", { name: "Upstream A", exact: true });
  await channel.focus();
  await page.keyboard.press("Enter");
  await page.getByLabel("Name", { exact: true }).fill("Unsaved channel");
  await page.evaluate(() => window.history.back());
  const dialog = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("Unsaved channel");
  await page.getByRole("tab", { name: "Channel capabilities", exact: true }).click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.getByRole("link", { name: "New capability" })).toBeVisible();
  await page.getByRole("tab", { name: "Channel settings" }).click();
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("Upstream A");
  await page.getByLabel("Name", { exact: true }).fill("Another draft");
  await page.getByRole("link", { name: "Back to channels" }).click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page).toHaveURL(new RegExp(`${origin.replace("?", "\\?")}$`));
});

test("nested capability editing has one parent link and no competing batch editor", async ({ page }) => {
  await login(page);
  await page.setViewportSize({ width: 390, height: 844 });
  const origin = `/admin/routing/channels?group=${E2E_ROUTING_GROUP_ID}`;
  await page.goto(origin);
  await page.getByRole("link", { name: "Upstream A", exact: true }).click();
  await page.getByRole("tab", { name: "Channel capabilities", exact: true }).click();
  await page.getByRole("link", { name: "Chat Completions", exact: true }).click();
  await expect(page.getByRole("button", { name: "Save capability" })).toBeVisible();
  await expect(page.getByRole("link", { name: /^Back to/ })).toHaveCount(1);
  await expect(page.getByRole("button", { name: /Batch edit capabilities/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Delete channel" })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
  await page.getByRole("link", { name: "Back to capabilities" }).click();
  await expect(page.getByRole("button", { name: /Batch edit capabilities/ })).toBeVisible();
  await page.getByRole("link", { name: "Back to channels" }).click();
  await expect(page).toHaveURL(new RegExp(`${origin.replace("?", "\\?")}$`));
});

test("direct detail entry has a safe parent and channel list deep links apply filters", async ({ page }) => {
  await login(page);
  await page.goto(`/admin/routing/logical-channels/${E2E_STANDARD_CHANNEL_ID}?returnTo=%2F%2Fevil.test`);
  await expect(page.getByRole("link", { name: "Back to channels" }))
    .toHaveAttribute("href", "/admin/routing/channels");
  await page.getByRole("link", { name: "Back to channels" }).click();
  await page.goto(`/admin/routing/channels?group=${E2E_ROUTING_GROUP_ID}`);
  await expect(page).toHaveURL(new RegExp(`/admin/routing/channels\\?group=${E2E_ROUTING_GROUP_ID}$`));
  await expect(page.getByRole("link", { name: "Upstream A", exact: true })).toBeVisible();
  await expect(page.getByRole("link", { name: "Personal Plus", exact: true })).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
});
