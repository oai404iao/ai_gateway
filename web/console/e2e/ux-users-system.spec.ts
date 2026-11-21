import { expect, test, type Locator, type Page } from "@playwright/test";
import type { InviteUserInput, SystemSettingsInput, UserBatchUpdateInput } from "../src/api/types";
import { SYSTEM_SETTINGS } from "../src/test/fixtures";
import { E2E_USER, mockConsoleApi } from "./mock-api";

const users = Array.from({ length: 45 }, (_, index) => ({
  ...E2E_USER,
  id: `00000000-0000-0000-0000-${String(index + 200).padStart(12, "0")}`,
  email: `user-${index + 1}@example.test`,
  display_name: `E2E User ${index + 1}`,
}));

async function signIn(page: Page, path: string) {
  await page.goto("/login");
  await page.getByLabel("Email").fill("admin@example.com");
  await page.getByLabel("Password", { exact: true }).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL("/");
  await page.goto(path);
}

async function expectReachable(locator: Locator) {
  await expect(locator).toBeInViewport({ ratio: 1 });
  await expect.poll(() => locator.evaluate((element) => {
    const { x, y, width, height } = element.getBoundingClientRect();
    return element.contains(document.elementFromPoint(x + width / 2, y + height / 2));
  })).toBe(true);
}

test("the short invitation dialog can be scrolled and completed with mouse or keyboard", async ({ page }) => {
  await mockConsoleApi(page);
  const invitations: InviteUserInput[] = [];
  await page.route("**/console/v1/users", (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    invitations.push(route.request().postDataJSON());
    return route.fulfill({ status: 201, json: {
      id: users[0].id,
      user_id: users[0].id,
      invitation_token: "synthetic-invitation-token",
      expires_at: "2099-01-02T00:00:00Z",
      correlation_id: users[0].id,
    } });
  });
  await signIn(page, "/admin/users");
  await page.setViewportSize({ width: 844, height: 390 });
  const openInvite = page.getByRole("button", { name: "Invite user" });
  await openInvite.focus();
  await page.keyboard.press("Enter");

  const dialog = page.getByRole("dialog", { name: "Invite user" });
  const title = dialog.getByRole("heading", { name: "Invite user" });
  const close = dialog.getByRole("button", { name: "Close" });
  const email = dialog.getByLabel("Email");
  const name = dialog.getByLabel("Display name");
  const submit = dialog.getByRole("button", { name: "Send invitation" });
  const cancel = dialog.getByRole("button", { name: "Cancel" });

  await expect(email).toBeFocused();
  await expectReachable(email);
  for (const field of [
    name,
    dialog.getByRole("combobox", { name: "Role" }),
    dialog.getByRole("combobox", { name: "User group" }),
    dialog.getByRole("combobox", { name: "API policy override" }),
    dialog.getByLabel("Initial balance"),
    cancel,
    submit,
  ]) {
    await page.keyboard.press("Tab");
    await expect(field).toBeFocused();
    await expectReachable(field);
  }
  await expectReachable(title);
  await expectReachable(close);
  await page.keyboard.press("Shift+Tab");
  await expect(cancel).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect(openInvite).toBeFocused();
  expect(invitations).toHaveLength(0);

  await openInvite.click();
  await expect(submit).not.toBeInViewport();
  await email.fill("invitee@example.test");
  await email.hover();
  await page.mouse.wheel(0, 800);
  await expectReachable(submit);
  await expectReachable(cancel);
  await submit.click();
  await expect(dialog.getByText("Display name is required.")).toBeInViewport();
  await expect(name).toBeFocused();
  expect(invitations).toHaveLength(0);

  await name.fill("E2E Invitee");
  await name.hover();
  await page.mouse.wheel(0, 800);
  await expectReachable(submit);
  await submit.click();
  await expect(page.getByRole("dialog", { name: "Invitation token" }).getByLabel("Invitation token"))
    .toHaveValue("synthetic-invitation-token");
  expect(invitations).toEqual([{
    email: "invitee@example.test",
    display_name: "E2E Invitee",
    role: "user",
    user_group_id: E2E_USER.user_group_id,
    initial_balance_amount: "0",
    default_api_key_policy_id: null,
  }]);
});

test("page selection and explicit all-user selection produce the confirmed batch scopes", async ({ page }) => {
  await mockConsoleApi(page);
  await page.route("**/console/v1/users", (route) =>
    route.request().method() === "GET" ? route.fulfill({ json: users }) : route.fallback(),
  );
  const batches: UserBatchUpdateInput[] = [];
  await page.route("**/console/v1/users/batch", (route) => {
    expect(route.request().method()).toBe("POST");
    const input: UserBatchUpdateInput = route.request().postDataJSON();
    batches.push(input);
    return route.fulfill({ json: {
      updated_ids: input.items.map(({ id }) => id),
      correlation_id: users[0].id,
    } });
  });
  await signIn(page, "/admin/users");

  const checkboxes = page.getByRole("table").getByRole("checkbox");
  const checked = page.getByRole("table").getByRole("checkbox", { checked: true });
  await expect(checkboxes).toHaveCount(20);
  await page.getByRole("button", { name: "Select 20 on this page" }).click();
  await expect(checked).toHaveCount(20);
  await expect(page.getByRole("status").getByText("20 users selected")).toBeVisible();

  await page.getByRole("button", { name: "Go to page 2" }).click();
  await expect(checkboxes).toHaveCount(20);
  await expect(checked).toHaveCount(0);
  await page.getByRole("checkbox", { name: "Select E2E User 21", exact: true }).check();
  await expect(page.getByRole("status").getByText("21 users selected")).toBeVisible();

  const submitBalanceBatch = async (expected: typeof users) => {
    await page.getByRole("button", { name: `Batch edit (${expected.length})` }).click();
    const dialog = page.getByRole("dialog", { name: "Batch edit users" });
    await expect(dialog.getByText(`Apply the selected changes atomically to ${expected.length} users.`)).toBeVisible();
    await dialog.getByRole("combobox").nth(3).click();
    await page.getByRole("option", { name: "Increase balance" }).click();
    await dialog.getByLabel("Balance amount").fill("5");
    await dialog.getByRole("button", { name: "Update users" }).click();
    await expect(dialog).toBeHidden();
    expect(batches.at(-1)).toEqual({
      items: expected.map(({ id, updated_at }) => ({ id, updated_at })),
      changes: { balance: { operation: "increase", amount: "5" } },
    });
  };

  await submitBalanceBatch(users.slice(0, 21));
  await page.getByRole("button", { name: "Go to page 3" }).click();
  await page.getByRole("button", { name: "Select 5 on this page" }).click();
  await expect(checked).toHaveCount(5);
  await page.getByRole("button", { name: "Clear selection" }).click();
  await expect(checked).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Batch edit (0)" })).toBeDisabled();
  await page.getByRole("button", { name: "Select all 45 users" }).click();
  await expect(page.getByRole("status").getByText("45 users selected")).toBeVisible();
  await submitBalanceBatch(users);
  expect(batches).toHaveLength(2);
});

test("system settings keep a stale draft through cancel and a failed reload", async ({ page }) => {
  await mockConsoleApi(page);
  const { updated_at, ...original } = SYSTEM_SETTINGS;
  const latest = {
    ...original,
    api_hosts: ["https://latest.example.test/v1"],
    upstream: { ...original.upstream, connect_timeout_seconds: 24 },
    request_retry: { ...original.request_retry, retryable_status_codes: [429, 503] },
  };
  let gets = 0;
  const writes: { etag: string | undefined; input: SystemSettingsInput }[] = [];
  const conflictResponse = Promise.withResolvers<void>();
  const latestResponse = Promise.withResolvers<void>();
  await page.route("**/console/v1/system/settings", async (route) => {
    const request = route.request();
    if (request.method() === "PUT") {
      writes.push({ etag: request.headers()["if-match"], input: request.postDataJSON() });
      if (writes.length === 1) {
        await conflictResponse.promise;
        return route.fulfill({ status: 409, json: { error: "conflict" } });
      }
      return route.fulfill({ json: { id: users[0].id, correlation_id: users[0].id } });
    }
    expect(request.method()).toBe("GET");
    gets += 1;
    if (gets === 1) return route.fulfill({ json: { ...original, updated_at }, headers: { ETag: '"old"' } });
    if (gets === 2) return route.fulfill({ status: 503, json: { error: "unavailable" } });
    if (gets === 3) await latestResponse.promise;
    return route.fulfill({
      json: { ...(writes[1]?.input ?? latest), updated_at: "2026-02-01T00:00:00Z" },
      headers: { ETag: writes.length === 2 ? '"saved"' : '"latest"' },
    });
  });
  await signIn(page, "/admin/system/upstream");
  const timeout = page.getByLabel("Connect timeout (seconds)");
  const save = page.getByRole("button", { name: "Save system settings" });
  const load = page.getByRole("button", { name: "Load latest version" });
  const confirm = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });

  await expect(timeout).toHaveValue("10");
  await timeout.fill("23");
  await save.click();
  try {
    await expect.poll(() => writes.length).toBe(1);
    await expect(timeout).toBeDisabled();
    await expect(timeout).toHaveValue("23");
  } finally {
    conflictResponse.resolve();
  }
  await expect(page.getByText("System settings changed elsewhere. Your draft was not saved.")).toBeVisible();
  expect(writes).toEqual([{
    etag: '"old"',
    input: { ...original, upstream: { ...original.upstream, connect_timeout_seconds: 23 } },
  }]);
  await expect(save).toBeDisabled();
  await expect(timeout).toBeEnabled();
  await expect(timeout).toHaveValue("23");
  await load.click();
  await confirm.getByRole("button", { name: "Cancel" }).click();
  await expect(confirm).toBeHidden();
  await expect(timeout).toHaveValue("23");
  expect(gets).toBe(1);

  await load.click();
  await confirm.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.getByText("The latest version could not be loaded. Your draft has been kept.")).toBeVisible();
  await expect(timeout).toHaveValue("23");
  await expect(save).toBeDisabled();
  expect(writes).toHaveLength(1);

  await load.click();
  await confirm.getByRole("button", { name: "Discard changes" }).click();
  try {
    await expect.poll(() => gets).toBe(3);
    await expect(timeout).toBeDisabled();
    await expect(load).toBeDisabled();
    await expect(timeout).toHaveValue("23");
  } finally {
    latestResponse.resolve();
  }
  await expect(timeout).toHaveValue("24");
  await expect(timeout).toBeEnabled();
  await timeout.fill("25");
  await save.click();
  await expect(page.getByText("System settings saved and applied.")).toBeVisible();
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual({
    etag: '"latest"',
    input: { ...latest, upstream: { ...latest.upstream, connect_timeout_seconds: 25 } },
  });
});
