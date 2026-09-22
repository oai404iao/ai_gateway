import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { assertRefreshRotation } from "./browser-checks.mjs";

const require = createRequire(new URL("../web/console/package.json", import.meta.url));
const { chromium, expect } = require("@playwright/test");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const data = JSON.parse(input);
const browser = await chromium.launch();
try {
  const context = await browser.newContext({ locale: "en-US" });
  const page = await context.newPage();
  page.setDefaultTimeout(15_000);
  await page.goto(`${data.console}/login`);
  await page.getByLabel("Email", { exact: true }).fill("system-e2e@example.test");
  await page.getByLabel("Password", { exact: true }).fill(data.password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/account$/);
  const cookies = await context.cookies();
  const refresh = cookies.find((cookie) => cookie.name === "__Host-ai_gateway_refresh");
  assert.ok(refresh?.httpOnly && refresh.secure && refresh.path === "/" &&
    refresh.sameSite === "Lax", "refresh cookie security attributes");
  const refreshing = page.waitForResponse((response) =>
    response.url().endsWith("/console/v1/auth/refresh") && response.request().method() === "POST",
  );
  await page.reload();
  assert.equal((await refreshing).status(), 200);
  await expect(page.getByRole("heading", { name: "Profile", exact: true })).toBeVisible();
  const rotated = (await context.cookies()).find((cookie) => cookie.name === refresh.name);
  assertRefreshRotation(refresh, rotated);
  const storage = await page.evaluate(() => Object.entries(localStorage));
  assert.ok(storage.every(([key]) => !/token|session|auth/i.test(key)), "auth must not persist in localStorage");

  const credentialUrl = `${data.console}/console/v1/routing/upstream-credentials/${data.upstream_credential_id}`;
  const credentialLoaded = page.waitForResponse((response) => response.url() === credentialUrl && response.request().method() === "GET");
  await page.goto(`${data.console}/admin/routing/upstream-credentials/${data.upstream_credential_id}`);
  const credentialEtag = (await credentialLoaded).headers().etag;
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
  assert.equal(credential.secret, data.upstream_rotated_secret);
  assert.equal(credential.channel_ids.length, 2);
  await page.goto(`${data.console}/admin/routing/logical-channels/${data.logical_channel_id}`);
  await expect(page.getByLabel("Credential", { exact: true })).toContainText("System E2E shared identity");
  await expect(page.getByLabel("Upstream API key", { exact: true })).toHaveCount(0);

  const protocolUrl = `${data.console}/console/v1${data.protocol_path}`;
  const loaded = page.waitForResponse((response) =>
    response.url() === protocolUrl && response.request().method() === "GET",
  );
  await page.goto(`${data.console}/admin${data.protocol_path}`);
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
  await page.reload();
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
    const logs = await response.json();
    return logs.length === 1 && logs[0].billed_at !== null;
  }, { timeout: 30_000 }).toBe(true);
  await page.goto(`${data.console}/admin/request-logs`);
  await expect(page.getByText("e2e-client", { exact: true }).first()).toBeVisible();
  console.log(JSON.stringify({
    id: "console-route-to-settlement", status: "passed",
    browser: browser.version(), refresh_rotated: true, etag_checked: true,
    shared_upstream_credential_rotated: true,
  }));
} finally {
  await browser.close();
}
