import { expect, test, type Page } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";
import { BUILT_IN_PLUGIN, PLUGIN, PLUGIN_JOB, PLUGIN_SETTINGS } from "../src/test/fixtures";
import type { PluginSettingsInput, PluginStateInput } from "../src/api/types";

const password = "correct-horse-battery-staple";

async function setup(page: Page) {
  await mockConsoleApi(page);
  const plugin = structuredClone(PLUGIN);
  const settings = structuredClone(PLUGIN_SETTINGS);
  const issuedTokens: string[] = [];
  let unusedToken: string | undefined;
  let uploadAccepted = false;
  await page.route((url) => /^\/console\/v1\/plugins(?:\/|$)/.test(url.pathname), async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const target = `${request.method()} ${path.replace(/^\/console\/v1\/plugins/, "")}`;
    const etag = `"plugin-${plugin.revision}"`;
    switch (target) {
      case "POST /reauth":
        if (request.postDataJSON()?.password !== password) {
          return route.fulfill({ status: 403, json: { error: "forbidden" } });
        }
        unusedToken = `native-operation-token-${issuedTokens.length + 1}`;
        issuedTokens.push(unusedToken);
        return route.fulfill({ json: { token: unusedToken, expires_at: "2099-01-01T00:00:00Z" } });
      case "GET ":
        return route.fulfill({ json: [BUILT_IN_PLUGIN, plugin] });
      case "GET /general":
        return route.fulfill({ json: BUILT_IN_PLUGIN, headers: { ETag: '"plugin-0"' } });
      case "GET /example":
        return route.fulfill({ json: plugin, headers: { ETag: etag } });
      case "GET /example/settings":
        return route.fulfill({ json: settings, headers: { ETag: etag } });
      case `GET /jobs/${PLUGIN_JOB.id}`:
        if (uploadAccepted) return route.fulfill({ json: PLUGIN_JOB });
        throw new Error("Plugin job read before upload");
      case "POST /install":
      case "PUT /example/settings":
      case "PUT /example/state":
        break;
      default:
        throw new Error(`Unhandled plugin request: ${request.method()} ${path}`);
    }
    if (!unusedToken || request.headers()["x-plugin-authorization"] !== unusedToken) {
      return route.fulfill({ status: 403, json: { error: "forbidden" } });
    }
    unusedToken = undefined;
    if (target === "POST /install") {
      if (request.headers()["content-type"] !== "application/octet-stream" || request.postData() !== "reviewed-package") {
        return route.fulfill({ status: 422, json: { error: "invalid_package" } });
      }
      uploadAccepted = true;
      return route.fulfill({ status: 202, json: { ...PLUGIN_JOB, status: "queued" } });
    }
    if (request.headers()["if-match"] !== etag) return route.fulfill({ status: 409, json: { error: "conflict" } });
    if (target === "PUT /example/settings") {
      const input = request.postDataJSON() as PluginSettingsInput;
      settings.values = input.values;
      settings.revision = ++plugin.revision;
      return route.fulfill({ json: { id: plugin.id, correlation_id: "settings-saved" } });
    }
    const input = request.postDataJSON() as PluginStateInput;
    const artifact = plugin.artifacts.find((artifact) => artifact.digest === input.artifact_digest);
    if (!artifact || typeof input.enabled !== "boolean") {
      return route.fulfill({ status: 422, json: { error: "invalid_input" } });
    }
    plugin.enabled = input.enabled;
    plugin.status = input.enabled ? "active" : "disabled";
    plugin.artifact_digest = artifact.digest;
    plugin.version = artifact.version;
    plugin.revision++;
    settings.artifact_digest = artifact.digest;
    settings.revision = plugin.revision;
    return route.fulfill({ json: { id: plugin.id, correlation_id: "state-saved" } });
  });
  await page.goto("/login");
  await page.getByLabel(/email/i).fill("admin@example.com");
  await page.getByLabel(/^password$/i).fill(password);
  await page.getByRole("button", { name: /sign in/i }).click();
  await expect(page).toHaveURL(/\/account/);
  await page.getByRole("link", { name: "Plugins", exact: true }).click();
  return issuedTokens;
}

async function authorize(page: Page, operation: string) {
  const dialog = page.getByRole("dialog", { name: "Authorize plugin operation" });
  await dialog.getByLabel("Current password").fill(password);
  await dialog.getByRole("button", { name: operation, exact: true }).click();
  await expect(dialog).toBeHidden();
}

test("generic plugin settings preserve all scalar types and survive reload", async ({ page }) => {
  const issuedTokens = await setup(page);
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
  const hasStoredSecret = () => page.evaluate((secrets) =>
    [localStorage, sessionStorage].some((storage) =>
      Object.entries(storage).some(([key, value]) => secrets.some((secret) =>
        key.includes(secret) || value.includes(secret)))), [...issuedTokens, password]);
  expect(issuedTokens).toHaveLength(1);
  expect(await hasStoredSecret()).toBe(false);
  await page.reload();
  await expect(page.getByLabel("Client label")).toHaveValue("browser-configured");
  await expect(page.getByLabel("Attempt limit")).toHaveValue("4");
  await expect(page.getByRole("switch", { name: "Compact payload" })).toBeChecked();
  await expect(page.getByRole("combobox", { name: "Protocol mode" })).toContainText("Standard");
  expect(await hasStoredSecret()).toBe(false);
});

test("plugin version switch and disable require explicit confirmation; general is read-only", async ({ page }) => {
  await setup(page);
  await page.getByRole("link", { name: "general", exact: true }).click();
  await expect(page.getByText("Built-in connector. Lifecycle and plugin settings are not editable.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Disable plugin" })).toHaveCount(0);
  await page.getByRole("link", { name: "Plugins", exact: true }).last().click();
  await page.getByRole("link", { name: "example", exact: true }).click();
  const installedVersion = page.getByRole("combobox", { name: "Installed version" });
  await expect(installedVersion).toContainText("1.0.0");
  await installedVersion.click();
  await page.getByRole("option", { name: /1.1.0/ }).click();
  await page.getByRole("button", { name: "Enable selected version" }).click();
  await authorize(page, "Enable selected version");
  await expect(page.getByText("Plugin state updated.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Enable selected version" })).toBeDisabled();
  await page.reload();
  await expect(installedVersion).toContainText("1.1.0");
  await expect(page.getByText("b".repeat(64), { exact: true })).toBeVisible();
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
