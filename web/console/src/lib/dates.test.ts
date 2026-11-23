import { afterEach, describe, expect, it } from "vitest";
import { setCurrentLocale } from "@/app/i18n";
import { formatDateTimeLocalInput } from "./dates";

afterEach(() => {
  setCurrentLocale("en-US");
});

describe("formatDateTimeLocalInput", () => {
  it("formats the local time in numeric input syntax with the requested precision", () => {
    const instant = new Date(2026, 2, 4, 5, 6, 47).toISOString();
    setCurrentLocale("zh-CN");
    expect(formatDateTimeLocalInput(instant)).toBe("2026-03-04T05:06");
    expect(formatDateTimeLocalInput(instant, true)).toBe("2026-03-04T05:06:47");
  });
});
