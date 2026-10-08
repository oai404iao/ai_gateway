import { expect, test, type Page } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";
import { BUILT_IN_PLUGIN, PLUGIN, PLUGIN_JOB, PLUGIN_SETTINGS } from "../src/test/fixtures";
import type { PluginSettingsInput, PluginStateInput } from "../src/api/types";

async function setup(page: Page) {
  await mockConsoleApi(page);
  const plugin = structuredClone(PLUGIN);
  const settings = structuredClone(PLUGIN_SETTINGS);
  await page.route("**/console/v1/plugins**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const etag = `"plugin-${plugin.revision}"`;
    if (path === "/console/v1/plugins/reauth") {
      return route.fulfill({ json: { token: "native-operation-token", expires_at: "2099-01-01T00:00:00Z" } });
    }
    if (request.method() !== "GET" && request.headers()["x-plugin-authorization"] !== "native-operation-token") {
      return route.fulfill({ status: 403, json: { error: "forbidden" } });
    }
    if (path === "/console/v1/plugins/install" || path === "/console/v1/plugins/discover") {
      return route.fulfill({ status: 202, json: { ...PLUGIN_JOB, status: "queued" } });
    }
    if (path.includes("/plugins/jobs/")) return route.fulfill({ json: PLUGIN_JOB });
    if (path === "/console/v1/plugins") return route.fulfill({ json: [BUILT_IN_PLUGIN, plugin] });
    if (path === "/console/v1/plugins/example/settings") {
      if (request.method() === "GET") return route.fulfill({ json: settings, headers: { ETag: etag } });
      if (request.headers()["if-match"] !== etag) return route.fulfill({ status: 409, json: { error: "conflict" } });
      const input = request.postDataJSON() as PluginSettingsInput;
      settings.values = input.values;
      settings.revision = ++plugin.revision;
      return route.fulfill({ json: { id: plugin.id, correlation_id: "settings-saved" } });
    }
    if (path === "/console/v1/plugins/example/state") {
      if (request.headers()["if-match"] !== etag) return route.fulfill({ status: 409, json: { error: "conflict" } });
      const input = request.postDataJSON() as PluginStateInput;
      plugin.enabled = input.enabled;
      plugin.status = input.enabled ? "active" : "disabled";
      plugin.artifact_digest = input.artifact_digest;
      plugin.version = plugin.artifacts.find((artifact) => artifact.digest === input.artifact_digest)?.version ?? null;
      plugin.revision++;
      return route.fulfill({ json: { id: plugin.id, correlation_id: "state-saved" } });
    }
    return route.fulfill({ json: path.endsWith("/general") ? BUILT_IN_PLUGIN : plugin, headers: { ETag: etag } });
  });
  await page.goto("/login");
  await page.getByLabel(/email/i).fill("admin@example.com");
  await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: /sign in/i }).click();
  await expect(page).toHaveURL(/\/account/);
  await page.getByRole("link", { name: "Plugins", exact: true }).click();
}

async function authorize(page: Page, operation: string) {
  const dialog = page.getByRole("dialog", { name: "Authorize plugin operation" });
  await dialog.getByLabel("Current password").fill("correct-horse-battery-staple");
  await dialog.getByRole("button", { name: operation, exact: true }).click();
  await expect(dialog).toBeHidden();
}

test("generic plugin settings preserve all scalar types and survive reload", async ({ page }) => {
  await setup(page);
  await page.getByRole("link", { name: "example", exact: true }).click();
  await expect(page.getByRole("heading", { name: "example", exact: true })).toBeVisible();
  await page.getByLabel("Client label").fill("browser-configured");
  await page.getByLabel("Attempt limit").fill("4");
  await page.getByRole("switch", { name: "Compact payload" }).click();
  await page.getByRole("combobox", { name: "Protocol mode" }).click();
  await page.getByRole("option", { name: "Standard", exact: true }).click();
  await page.getByRole("button", { name: "Save plugin settings" }).click();
  const saved = page.waitForRequest((request) => request.method() === "PUT" && request.url().endsWith("/plugins/example/settings"));
  await authorize(page, "Save plugin settings");
  expect((await saved).postDataJSON()).toEqual({
    schema_version: 1, values: { client_label: "browser-configured", attempts: 4, compact: true, mode: "standard" },
  });
  await page.reload();
  await expect(page.getByLabel("Client label")).toHaveValue("browser-configured");
  await expect(page.getByRole("switch", { name: "Compact payload" })).toBeChecked();
  await expect(page.getByRole("combobox", { name: "Protocol mode" })).toContainText("Standard");
  const storage = await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  expect(storage).not.toContain("native-operation-token");
  expect(storage).not.toContain("correct-horse-battery-staple");
});

test("plugin version switch and disable require explicit confirmation; general is read-only", async ({ page }) => {
  await setup(page);
  await page.getByRole("link", { name: "general", exact: true }).click();
  await expect(page.getByText("Built-in connector. Lifecycle and plugin settings are not editable.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Disable plugin" })).toHaveCount(0);
  await page.getByRole("link", { name: "Plugins", exact: true }).last().click();
  await page.getByRole("link", { name: "example", exact: true }).click();
  await page.getByRole("combobox", { name: "Installed version" }).click();
  await page.getByRole("option", { name: /1.1.0/ }).click();
  await page.getByRole("button", { name: "Enable selected version" }).click();
  await authorize(page, "Enable selected version");
  await expect(page.getByText("Plugin state updated.")).toBeVisible();
  await page.getByRole("button", { name: "Disable plugin" }).click();
  await authorize(page, "Disable plugin");
  await expect(page.getByText("disabled", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Disable plugin" })).toBeDisabled();
});

test("raw upload exposes a resumable asynchronous job and remains usable on a narrow screen", async ({ page }) => {
  await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByLabel("Plugin package").setInputFiles({ name: "reviewed.tar.gz", mimeType: "application/gzip", buffer: Buffer.from("reviewed-package") });
  await page.getByRole("button", { name: "Install plugin", exact: true }).click();
  const uploaded = page.waitForRequest((request) => request.url().endsWith("/plugins/install"));
  await authorize(page, "Install plugin");
  const request = await uploaded;
  expect(request.headers()["content-type"]).toBe("application/octet-stream");
  expect(request.postData()).toBe("reviewed-package");
  await expect(page).toHaveURL(new RegExp(`job=${PLUGIN_JOB.id}`));
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
