import { describe, expect, it } from "vitest";
import {
  formatBoolean,
  formatBytes,
  formatExactTokens,
  formatDurationMs,
  formatEstimatedQuotaTotal,
  formatList,
  formatTokens,
  formatUsd,
  truncate,
} from "@/lib/formatters";

describe("formatters", () => {
  it("formats USD amounts", () => {
    expect(formatUsd("1.50000")).toBe("1.5 USD");
    expect(formatUsd("-0.00000")).toBe("0 USD");
    expect(formatUsd("19.999995")).toBe("20 USD");
    expect(formatUsd("0.00001")).toBe("0.00001 USD");
    expect(formatUsd("0.000009")).toBe("<0.00001 USD");
    expect(formatUsd("0.0000099999999999999999999999")).toBe("<0.00001 USD");
    expect(formatUsd("-0.000001")).toBe(">-0.00001 USD");
    expect(formatUsd("9007199254740993.123455")).toBe("9,007,199,254,740,993.12346 USD");
    expect(formatUsd(null)).toBe("—");
  });

  it("estimates total quota only from valid window observations", () => {
    expect(formatEstimatedQuotaTotal("3.2174", 42)).toBe("7.66048 USD");
    expect(formatEstimatedQuotaTotal("0", 50)).toBe("0 USD");
    expect(formatEstimatedQuotaTotal(null, 42)).toBe("—");
    expect(formatEstimatedQuotaTotal("3.2174", null)).toBe("—");
    expect(formatEstimatedQuotaTotal("3.2174", 0)).toBe("—");
    expect(formatEstimatedQuotaTotal("3.2174", 101)).toBe("—");
    expect(formatEstimatedQuotaTotal("not-a-decimal", 42)).toBe("—");
    expect(formatEstimatedQuotaTotal("-1", 42)).toBe("—");
  });

  it("formats durations", () => {
    expect(formatDurationMs(42)).toBe("42 ms");
    expect(formatDurationMs(1500)).toBe("1.5 s");
    expect(formatDurationMs(null)).toBe("—");
  });

  it("formats byte counts", () => {
    expect(formatBytes(2_097_152)).toBe("2 MiB");
    expect(formatBytes(null)).toBe("—");
  });

  it("formats tokens and lists", () => {
    expect(formatExactTokens(1234)).toBe("1,234");
    expect(formatTokens(null)).toBe("—");
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(999)).toBe("999");
    expect(formatTokens(1_000)).toBe("1K");
    expect(formatTokens(1_250)).toBe("1.25K");
    expect(formatTokens(999_994)).toBe("999.99K");
    expect(formatTokens(999_995)).toBe("1M");
    expect(formatTokens(12_500_000)).toBe("12.5M");
    expect(formatTokens(999_995_000)).toBe("1B");
    expect(formatTokens(2_000_000_000)).toBe("2B");
    expect(formatList(["a", "b"])).toBe("a, b");
    expect(formatList([], "none")).toBe("none");
  });

  it("formats booleans and truncates", () => {
    expect(formatBoolean(true)).toBe("Yes");
    expect(formatBoolean(false)).toBe("No");
    expect(truncate("abcdefghij", 5)).toBe("abcd…");
  });
});
