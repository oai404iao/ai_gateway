import { describe, expect, it } from "vitest";
import { returnPathLabel, safeReturnPath, withReturnTo } from "./page-navigation";

describe("page navigation", () => {
  it.each(["https://evil.test", "//evil.test", "/\\evil.test", "/admin/../../login",
    "/admin/\nusers", "javascript:alert(1)", "/api-keys", "/console/v1/users"])(
    "rejects unsafe or out-of-scope admin return %s", (value) => {
      expect(safeReturnPath(value, "/admin/users")).toBe("/admin/users");
    },
  );
  it("keeps internal query state without letting personal pages return to admin routes", () => {
    expect(safeReturnPath("/api-keys?page=2&pageSize=10", "/api-keys"))
      .toBe("/api-keys?page=2&pageSize=10");
    expect(safeReturnPath("/admin/users", "/api-keys")).toBe("/api-keys");
    expect(safeReturnPath("/admin/routing/channels?group=id&page=3", "/admin/users"))
      .toBe("/admin/routing/channels?group=id&page=3");
  });
  it("preserves a nested parent while opening an inline editor", () => {
    const parent = "/admin/routing/channels?group=id&page=2";
    const channel = withReturnTo("/admin/routing/logical-channels/id?view=capabilities", parent);
    const editor = new URL(withReturnTo("/admin/routing/logical-channels/id?view=capabilities&capability=new", channel), "https://console.invalid");
    expect(editor.searchParams.get("returnTo")).toBe(parent);
    expect(editor.searchParams.get("capability")).toBe("new");
  });
  it("labels the actual return destination", () => {
    expect(returnPathLabel("/admin/routing/channels?view=groups")).toBe("Back to channel groups");
    expect(returnPathLabel("/admin/routing/logical-channels/id?view=capabilities")).toBe("Back to channel");
    expect(returnPathLabel("/admin/models?view=prices")).toBe("Back to price sync");
  });
});
