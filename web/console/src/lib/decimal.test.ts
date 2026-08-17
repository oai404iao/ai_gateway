import { describe, expect, it } from "vitest";
import {
  isNonNegativeDecimal,
  isNonNegativeRustDecimal,
  multiplyDecimal,
} from "@/lib/decimal";

describe("decimal helpers", () => {
  it("multiplies exact price strings without floating-point artifacts", () => {
    expect(multiplyDecimal("0.15", "0.5")).toBe("0.075");
    expect(multiplyDecimal("0.1", "0.2")).toBe("0.02");
    expect(multiplyDecimal("123456789.123456789", "2")).toBe("246913578.246913578");
    expect(multiplyDecimal("1.2300", "1")).toBe("1.23");
    expect(multiplyDecimal("0", "999")).toBe("0");
  });

  it("rounds half-up to twelve fractional digits by default", () => {
    expect(multiplyDecimal("0.0000000000005", "1")).toBe("0.000000000001");
    expect(multiplyDecimal("0.0000000000004", "1")).toBe("0");
    expect(multiplyDecimal("1.2345", "1", 3)).toBe("1.235");
  });

  it("rejects malformed or negative inputs", () => {
    expect(isNonNegativeDecimal("1.25")).toBe(true);
    expect(isNonNegativeDecimal("-1")).toBe(false);
    expect(isNonNegativeDecimal(".5")).toBe(false);
    expect(isNonNegativeDecimal("01")).toBe(false);
    expect(() => multiplyDecimal("NaN", "2")).toThrow();
  });

  it("recognizes the rust_decimal coefficient and scale limits", () => {
    expect(isNonNegativeRustDecimal("79228162514264337593543950335")).toBe(true);
    expect(isNonNegativeRustDecimal("79228162514264337593543950336")).toBe(false);
    expect(isNonNegativeRustDecimal("0.1234567890123456789012345678")).toBe(true);
    expect(isNonNegativeRustDecimal("0.12345678901234567890123456789")).toBe(false);
  });
});
