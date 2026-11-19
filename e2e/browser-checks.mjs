import assert from "node:assert/strict";

export function assertRefreshRotation(previous, current) {
  // Assertion diagnostics are uploaded: never attach cookie values as actual/expected.
  assert.ok(Boolean(current) && current.value !== previous.value, "refresh cookie must rotate");
}

export function credentialsPersisted({ secrets, local = localStorage, session = sessionStorage }) {
  return [local, session].some((storage) => Object.entries(storage).some(([key, value]) =>
    secrets.some((secret) => secret && (key.includes(secret) || value.includes(secret)))));
}
