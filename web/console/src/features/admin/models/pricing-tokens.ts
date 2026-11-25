import { formatExactTokens, formatTokens } from "@/lib/formatters";

/** Keep billing thresholds and denominators unambiguous when their compact label rounds. */
export function formatPricingTokens(value: number): string {
  const compact = formatTokens(value);
  if (!Number.isInteger(value) || value < 1_000) return compact;
  const divisor = value >= 1e9 ? 1e9 : value >= 1e6 ? 1e6 : 1e3;
  return value % (divisor / 100) === 0 ? compact : `${compact} (${formatExactTokens(value)})`;
}
