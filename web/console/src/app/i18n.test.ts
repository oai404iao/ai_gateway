import { describe, expect, it } from "vitest";
import { translateFor } from "@/app/i18n";

describe("Console i18n", () => {
  it("uses the English message when a translation is missing", () => {
    expect(translateFor("zh-CN", "Unknown message {id}", { id: 0 })).toBe("Unknown message 0");
  });

  it("interpolates values in the translated message", () => {
    expect(translateFor("zh-CN", "Session {id}", { id: "000000a1" })).toBe("会话 000000a1");
  });
});
