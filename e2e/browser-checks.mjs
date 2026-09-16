import assert from "node:assert/strict";

export function assertRefreshRotation(previous, current) {
  // Assertion diagnostics are uploaded: never attach cookie values as actual/expected.
  assert.ok(Boolean(current) && current.value !== previous.value, "refresh cookie must rotate");
}
