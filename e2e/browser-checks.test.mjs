import assert from "node:assert/strict";
import { inspect } from "node:util";
import test from "node:test";
import { assertRefreshRotation, credentialsPersisted } from "./browser-checks.mjs";

test("rotation failures do not expose refresh values in diagnostic artifacts", () => {
  const cookie = { value: "synthetic-private-refresh-token" };
  for (const current of [cookie, undefined]) {
    assert.throws(() => assertRefreshRotation(cookie, current), (error) => {
      assert.ok(inspect(error).includes("refresh cookie must rotate"));
      assert.ok(!inspect(error).includes(cookie.value));
      return true;
    });
  }
  assertRefreshRotation(cookie, { value: "different" });
});

test("each storage is checked for session credentials under ordinary keys or in values", () => {
  const secrets = ["synthetic-access", "synthetic-refresh", "synthetic-plugin-authorization"];
  const local = { "console-theme": "dark", preference: "local" };
  const session = { preference: "session" };
  assert.equal(credentialsPersisted({ secrets, local, session }), false);
  assert.equal(credentialsPersisted({
    secrets, local: { ...local, preference: JSON.stringify({ value: secrets[0] }) }, session,
  }), true);
  assert.equal(credentialsPersisted({ secrets, local, session: { preference: secrets[1] } }), true);
  assert.equal(credentialsPersisted({ secrets, local, session: { [secrets[2]]: "value" } }), true);
});
