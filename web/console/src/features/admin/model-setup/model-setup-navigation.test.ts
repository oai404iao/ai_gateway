import { describe, expect, it } from "vitest";
import {
  adminPath,
  safeAdminReturnPath,
  validResourceId,
} from "@/features/admin/model-setup/model-setup-navigation";

describe("model setup navigation", () => {
  it("accepts only UUID resource identifiers", () => {
    expect(
      validResourceId("00000000-0000-4000-8000-000000000001"),
    ).toBe("00000000-0000-4000-8000-000000000001");
    expect(validResourceId("../../../models")).toBeNull();
    expect(validResourceId(null)).toBeNull();
  });

  it("keeps return navigation inside administrator routes", () => {
    expect(safeAdminReturnPath("/admin/models?q=chat", "/admin/models")).toBe("/admin/models?q=chat");
    expect(safeAdminReturnPath("/account", "/admin/models")).toBe("/admin/models");
  });

  it("encodes setup query parameters without serializing empty values", () => {
    const params = { copyFrom: "00000000-0000-4000-8000-000000000001", returnTo: "/admin/models?q=chat&page=2" };
    const path = adminPath("/admin/models/new", { ...params, ignored: null });
    expect(path.startsWith("/admin/models/new?")).toBe(true);
    const query = new URL(path, "https://console.example").searchParams;
    expect([...query].sort()).toEqual(Object.entries(params).sort());
  });
});
