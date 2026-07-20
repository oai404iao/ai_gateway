import { afterEach, describe, expect, it } from "vitest";
import { setCurrentLocale } from "@/app/i18n";
import { formatDateTimeLocalInput } from "./dates";

afterEach(() => {
  setCurrentLocale("en-US");
});

describe("formatDateTimeLocalInput", () => {
  it("keeps the required numeric datetime-local syntax in the Chinese UI", () => {
    const value = "2026-03-04T05:06:00Z";
    const date = new Date(value);
    const expected = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(
      2,
      "0",
    )}-${String(date.getDate()).padStart(2, "0")}T${String(date.getHours()).padStart(
      2,
      "0",
    )}:${String(date.getMinutes()).padStart(2, "0")}`;

    setCurrentLocale("zh-CN");

    expect(formatDateTimeLocalInput(value)).toBe(expected);
  });
});
