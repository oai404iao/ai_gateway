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
    expect(
      safeAdminReturnPath(
        "/admin/models?q=chat",
        "/admin/models",
      ),
    ).toBe("/admin/models?q=chat");
    expect(
      safeAdminReturnPath("//evil.example/path", "/admin/models"),
    ).toBe("/admin/models");
    expect(
      safeAdminReturnPath("https://evil.example/path", "/admin/models"),
    ).toBe("/admin/models");
    expect(safeAdminReturnPath("/account", "/admin/models")).toBe(
      "/admin/models",
    );
  });

  it("encodes setup query parameters without serializing empty values", () => {
    expect(
      adminPath("/admin/models/new", {
        copyFrom: "00000000-0000-4000-8000-000000000001",
        returnTo: "/admin/models?q=chat",
        ignored: null,
      }),
    ).toBe(
      "/admin/models/new?copyFrom=00000000-0000-4000-8000-000000000001&returnTo=%2Fadmin%2Fmodels%3Fq%3Dchat",
    );
  });
});
