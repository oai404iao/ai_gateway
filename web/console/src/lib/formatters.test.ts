import { describe, expect, it } from "vitest";
import {
  formatBoolean,
  formatBytes,
  formatCompactTokens,
  formatDurationMs,
  formatEstimatedQuotaTotal,
  formatList,
  formatTokens,
  formatUsd,
  truncate,
} from "@/lib/formatters";

describe("formatters", () => {
  it("formats USD amounts", () => {
    expect(formatUsd("1.5")).toBe("1.50 USD");
    expect(formatUsd(null)).toBe("—");
  });

  it("estimates total quota only from valid window observations", () => {
    expect(formatEstimatedQuotaTotal("3.2174", 42)).toBe("7.660476 USD");
    expect(formatEstimatedQuotaTotal("0", 50)).toBe("0.00 USD");
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
    expect(formatTokens(1234)).toBe("1,234");
    expect(formatCompactTokens(999)).toBe("999");
    expect(formatCompactTokens(1_250)).toBe("1.25K");
    expect(formatCompactTokens(12_500_000)).toBe("12.5M");
    expect(formatCompactTokens(2_000_000_000)).toBe("2B");
    expect(formatList(["a", "b"])).toBe("a, b");
    expect(formatList([], "none")).toBe("none");
  });

  it("formats booleans and truncates", () => {
    expect(formatBoolean(true)).toBe("Yes");
    expect(formatBoolean(false)).toBe("No");
    expect(truncate("abcdefghij", 5)).toBe("abcd…");
  });
});
