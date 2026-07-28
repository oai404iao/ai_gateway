import { describe, expect, it } from "vitest";
import {
  describeSessionDevice,
  shortSessionId,
} from "@/features/sessions/session-display";

describe("describeSessionDevice", () => {
  it("recognizes common browser and platform combinations", () => {
    expect(
      describeSessionDevice(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15",
        "Unknown browser",
      ),
    ).toEqual({ label: "Safari 18 · macOS", kind: "desktop" });
    expect(
      describeSessionDevice(
        "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/126.0 Mobile Safari/537.36",
        "Unknown browser",
      ),
    ).toEqual({ label: "Chrome 126 · Android", kind: "mobile" });
    expect(describeSessionDevice("curl/8.7.1", "Unknown browser")).toEqual({
      label: "curl 8",
      kind: "terminal",
    });
  });

  it("falls back safely when metadata is unavailable", () => {
    expect(describeSessionDevice(null, "Unknown browser")).toEqual({
      label: "Unknown browser",
      kind: "unknown",
    });
    expect(shortSessionId("00000000-0000-0000-0000-00000000abcd")).toBe("0000abcd");
  });
});
