import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { assertRefreshRotation, credentialsPersisted } from "./browser-checks.mjs";

const require = createRequire(new URL("../web/console/package.json", import.meta.url));
const { chromium, expect } = require("@playwright/test");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const data = JSON.parse(input);
const browser = await chromium.launch();
try {
  const context = await browser.newContext({ locale: "en-US" });
  const page = await context.newPage();
  const secrets = [data.password];
  function watchPost(path) {
    return page.waitForResponse((response) => response.url() === `${data.console}/console/v1${path}` &&
      response.request().method() === "POST");
  }
  async function rememberToken(response, field) {
    assert.equal(response.status(), 200, "real browser authorization response");
    const token = (await response.json())[field];
    assert.ok(typeof token === "string" && token.length > 0, "authorization response must issue a token");
    secrets.push(token);
    return token;
  }
  async function assertMemoryOnly() {
    assert.ok(!(await page.evaluate(credentialsPersisted, { secrets })),
      "browser storage must not persist session or reauthorization credentials");
  }
  async function refreshCookie() {
    const cookie = (await context.cookies()).find(({ name }) => name === "__Host-ai_gateway_refresh");
    assert.ok(cookie?.httpOnly && cookie.secure && cookie.path === "/" && cookie.sameSite === "Lax",
      "refresh cookie security attributes");
    secrets.push(cookie.value);
    return cookie;
  }
  async function navigateWithRefresh(url) {
    const refreshing = watchPost("/auth/refresh");
    if (url) await page.goto(url);
    else await page.reload();
    await rememberToken(await refreshing, "access_token");
    const cookie = await refreshCookie();
    await assertMemoryOnly();
    return cookie;
  }
  async function assertOverview() {
    await expect(page.getByRole("heading", { level: 1, name: "Gateway overview" })).toBeVisible();
    await expect(page.getByRole("group", { name: "Chart metric" }).getByRole("button", { name: /^Requests/ })).toBeVisible();
    await expect(page.getByRole("main").getByRole("alert")).toHaveCount(0);
  }
  page.setDefaultTimeout(15_000);
  await page.goto(`${data.console}/login`);
  await page.getByLabel("Email", { exact: true }).fill("system-e2e@example.test");
  await page.getByLabel("Password", { exact: true }).fill(data.password);
  const login = watchPost("/auth/login");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await rememberToken(await login, "access_token");
  await expect(page).toHaveURL(`${data.console}/`);
  await assertOverview();
  const refresh = await refreshCookie();
  await assertMemoryOnly();
  const rotated = await navigateWithRefresh();
  await assertOverview();
  await expect(page.getByRole("navigation", { name: "Console navigation" }).getByRole("link", { name: "Overview", exact: true })).toHaveAttribute("aria-current", "page");
  assertRefreshRotation(refresh, rotated);

  const pluginUrl = `${data.console}/console/v1/plugins/codex`;
  const pluginSettingsUrl = `${pluginUrl}/settings`;
  await page.getByRole("navigation", { name: "Console navigation" }).getByRole("link", { name: "Management" }).click();
  const management = page.getByRole("navigation", { name: "Management navigation" });
  await management.getByRole("button", { name: "System settings", exact: true }).click();
  await management.getByRole("link", { name: "Plugins", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Plugins", exact: true })).toBeVisible();
  await page.getByLabel("Plugin package", { exact: true }).setInputFiles(data.plugin_archive);
  await page.getByRole("button", { name: "Install plugin", exact: true }).click();
  const pluginInstalled = await confirmPluginWrite("Install plugin", `${data.console}/console/v1/plugins/install`, "POST");
  assert.equal(pluginInstalled.request().headers()["content-type"], "application/octet-stream");
  await expect(page.getByText("succeeded", { exact: true })).toBeVisible();
  const settingsLoaded = page.waitForResponse((response) =>
    response.url() === pluginSettingsUrl && response.request().method() === "GET",
  );
  await page.getByRole("table").getByRole("link", { name: "codex", exact: true }).click();
  const settingsEtag = (await settingsLoaded).headers().etag;
  assert.ok(settingsEtag, "plugin settings GET must provide ETag");
  const clientVersion = page.locator("#plugin-setting-client_version");
  await expect(clientVersion).toHaveValue("0.146.0-e2e");
  await clientVersion.fill("0.146.0-browser");

  async function confirmPluginWrite(operation, url, method = "PUT") {
    const dialog = page.getByRole("dialog", { name: "Authorize plugin operation" });
    await dialog.getByLabel("Current password", { exact: true }).fill(data.password);
    const saving = page.waitForResponse((response) =>
      response.url() === url && response.request().method() === method,
    );
    const reauthorizing = watchPost("/plugins/reauth");
    await dialog.getByRole("button", { name: operation, exact: true }).click();
    const authorization = await rememberToken(await reauthorizing, "token");
    const saved = await saving;
    assert.equal(saved.status(), method === "POST" ? 202 : 200, "real native plugin mutation");
    assert.ok(saved.request().headers()["x-plugin-authorization"] === authorization,
      "native mutation must use this reauthorization");
    await expect(dialog).toBeHidden();
    await assertMemoryOnly();
    return saved;
  }

  await page.getByRole("button", { name: "Save plugin settings", exact: true }).click();
  const pluginSaved = await confirmPluginWrite("Save plugin settings", pluginSettingsUrl);
  assert.equal(pluginSaved.request().headers()["if-match"], settingsEtag);
  await navigateWithRefresh();
  await expect(clientVersion).toHaveValue("0.146.0-browser");
  await page.getByRole("button", { name: "Disable plugin", exact: true }).click();
  await confirmPluginWrite("Disable plugin", `${pluginUrl}/state`);
  await expect(page.getByText("disabled", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Disable plugin", exact: true })).toBeDisabled();
  await page.getByRole("button", { name: "Enable selected version", exact: true }).click();
  await confirmPluginWrite("Enable selected version", `${pluginUrl}/state`);
  await expect(page.getByText("active", { exact: true })).toBeVisible();
  await navigateWithRefresh();
  await expect(clientVersion).toHaveValue("0.146.0-browser");
  const systemSettings = await context.request.get(`${data.console}/console/v1/system/settings`, {
    headers: { Authorization: `Bearer ${data.token}` },
  });
  assert.equal(systemSettings.status(), 200);
  assert.ok(!Object.hasOwn(await systemSettings.json(), "codex"), "provider settings must be independent");
  await assertMemoryOnly();

  const credentialUrl = `${data.console}/console/v1/routing/upstream-credentials/${data.upstream_credential_id}`;
  const credentialLoaded = page.waitForResponse((response) => response.url() === credentialUrl && response.request().method() === "GET");
  await navigateWithRefresh(`${data.console}/admin/routing/upstream-credentials/${data.upstream_credential_id}`);
  const credentialEtag = (await credentialLoaded).headers().etag;
  assert.ok(credentialEtag, "credential GET must provide ETag");
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("System E2E shared identity");
  await expect(page.getByLabel("Credential secret", { exact: true })).toHaveValue("");
  await expect(page.getByRole("button", { name: "Delete credential", exact: true })).toBeDisabled();
  await expect(page.getByRole("link", { name: "system-e2e-disabled-reference", exact: true })).toBeVisible();
  await page.getByLabel("Credential secret", { exact: true }).fill(data.upstream_rotated_secret);
  const credentialSaving = page.waitForResponse((response) => response.url() === credentialUrl && response.request().method() === "PUT");
  await page.getByRole("button", { name: "Save credential", exact: true }).click();
  const credentialSaved = await credentialSaving;
  assert.equal(credentialSaved.status(), 200);
  assert.equal(credentialSaved.request().headers()["if-match"], credentialEtag);
  const credentialRead = await context.request.get(credentialUrl, { headers: { Authorization: `Bearer ${data.token}` } });
  const credential = await credentialRead.json();
  assert.ok(credential.secret === data.upstream_rotated_secret, "rotated secret must persist");
  assert.equal(credential.channel_ids.length, 2);
  await navigateWithRefresh(`${data.console}/admin/routing/logical-channels/${data.logical_channel_id}`);
  await expect(page.getByLabel("Credential", { exact: true })).toContainText("System E2E shared identity");
  await expect(page.getByLabel("Upstream API key", { exact: true })).toHaveCount(0);

  const protocolUrl = `${data.console}/console/v1${data.protocol_path}`;
  const loaded = page.waitForResponse((response) =>
    response.url() === protocolUrl && response.request().method() === "GET",
  );
  await navigateWithRefresh(`${data.console}/admin${data.protocol_path}`);
  const etag = (await loaded).headers().etag;
  assert.ok(etag, "protocol GET must provide ETag");
  const model = page.getByRole("combobox", { name: "Upstream model for tier 1 row 1" });
  await expect(model).toHaveValue("e2e-before");
  await model.fill("e2e-wire");
  await page.getByRole("option", { name: "e2e-wire", exact: true }).click();
  await page.getByRole("spinbutton", { name: "Weight for tier 1 row 1" }).fill("7");
  const saving = page.waitForResponse((response) =>
    response.url() === protocolUrl && response.request().method() === "PUT",
  );
  await page.getByRole("button", { name: "Save operation rule", exact: true }).click();
  const saved = await saving;
  assert.equal(saved.status(), 200);
  assert.equal(saved.request().headers()["if-match"], etag);
  await navigateWithRefresh();
  await expect(model).toHaveValue("e2e-wire");
  await expect(page.getByRole("spinbutton", { name: "Weight for tier 1 row 1" })).toHaveValue("7");

  const response = await context.request.post(`${data.public}/v1/responses`, {
    headers: { Authorization: `Bearer ${data.api_key}` },
    data: { model: "e2e-client", input: "Reply E2E_TEXT_OK", stream: false },
  });
  assert.equal(response.status(), 200, "real forwarding after browser route edit");
  assert.equal((await response.json()).output[0].content[0].text, "E2E_TEXT_OK");
  await expect.poll(async () => {
    const response = await context.request.get(
      `${data.console}/console/v1/request-logs?api_key_id=${data.api_key_id}`,
      { headers: { Authorization: `Bearer ${data.token}` } },
    );
    assert.equal(response.status(), 200);
    const { items: logs } = await response.json();
    return logs.length === 1 && logs[0].billed_at !== null;
  }, { timeout: 30_000 }).toBe(true);
  await navigateWithRefresh(`${data.console}/admin/request-logs`);
  const modelCell = page.getByRole("cell").filter({ has: page.getByText("e2e-client", { exact: true }) });
  await expect(modelCell.getByText("e2e-wire", { exact: true })).toBeVisible();
  const filters = page.getByRole("button", { name: /^Filters \(/ });
  const modelFilter = page.getByRole("combobox", { name: "Requested model", exact: true });
  await filters.click();
  await modelFilter.fill("e2e-wire");
  await modelFilter.press("Tab");
  const wireOnly = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.pathname === "/console/v1/request-logs" && url.searchParams.get("model") === "e2e-wire";
  });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  assert.deepEqual((await (await wireOnly).json()).items, []);
  await filters.click();
  await modelFilter.fill("e2e-c");
  await page.getByRole("option", { name: "e2e-client", exact: true }).click();
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(modelCell.getByText("e2e-wire", { exact: true })).toBeVisible();
  console.log(JSON.stringify({
    id: "console-route-to-settlement", status: "passed",
    browser: browser.version(), refresh_rotated: true, etag_checked: true,
    shared_upstream_credential_rotated: true,
    plugin_uploaded: true, plugin_settings_etag_checked: true, plugin_disabled_and_reenabled: true,
  }));
} finally {
  await browser.close();
}
