import assert from "node:assert/strict";
import { inspect } from "node:util";
import test from "node:test";
import { assertRefreshRotation } from "./browser-checks.mjs";

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
