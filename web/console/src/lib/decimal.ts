const NON_NEGATIVE_DECIMAL = /^(?:0|[1-9][0-9]*)(?:\.[0-9]+)?$/;
const MAX_DECIMAL_INPUT_LENGTH = 80;
const MAX_RUST_DECIMAL_COEFFICIENT = 79_228_162_514_264_337_593_543_950_335n;

interface ParsedDecimal {
  coefficient: bigint;
  scale: number;
}

export function isNonNegativeDecimal(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= MAX_DECIMAL_INPUT_LENGTH &&
    NON_NEGATIVE_DECIMAL.test(value)
  );
}

export function isNonNegativeRustDecimal(value: string): boolean {
  if (!isNonNegativeDecimal(value)) return false;
  const parsed = parseDecimal(value);
  return (
    parsed.scale <= 28 &&
    parsed.coefficient <= MAX_RUST_DECIMAL_COEFFICIENT
  );
}

function parseDecimal(value: string): ParsedDecimal {
  if (!isNonNegativeDecimal(value)) {
    throw new Error("Invalid non-negative decimal.");
  }
  const [whole, fraction = ""] = value.split(".");
  return {
    coefficient: BigInt(`${whole}${fraction}`),
    scale: fraction.length,
  };
}

function powerOfTen(exponent: number): bigint {
  return 10n ** BigInt(exponent);
}

function formatDecimal(coefficient: bigint, scale: number): string {
  if (coefficient === 0n) return "0";
  let digits = coefficient.toString();
  if (scale === 0) return digits;
  if (digits.length <= scale) {
    digits = digits.padStart(scale + 1, "0");
  }
  const split = digits.length - scale;
  const whole = digits.slice(0, split);
  const fraction = digits.slice(split).replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : whole;
}

/**
 * Multiplies two non-negative decimal strings without binary floating-point
 * conversion. Results are rounded half-up to the requested fractional scale.
 */
export function multiplyDecimal(
  left: string,
  right: string,
  maximumFractionDigits = 12,
): string {
  if (!Number.isSafeInteger(maximumFractionDigits) || maximumFractionDigits < 0) {
    throw new Error("Invalid decimal scale.");
  }
  const leftValue = parseDecimal(left);
  const rightValue = parseDecimal(right);
  let coefficient = leftValue.coefficient * rightValue.coefficient;
  let scale = leftValue.scale + rightValue.scale;

  if (scale > maximumFractionDigits) {
    const divisor = powerOfTen(scale - maximumFractionDigits);
    const quotient = coefficient / divisor;
    const remainder = coefficient % divisor;
    coefficient = remainder * 2n >= divisor ? quotient + 1n : quotient;
    scale = maximumFractionDigits;
  }

  return formatDecimal(coefficient, scale);
}
