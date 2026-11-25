/** Formatting helpers for Console display values. Decimals arrive as strings. */

import { currentLocale, translate } from "@/app/i18n";

type Amount = string | number | null | undefined;

/** Use without a suffix when the heading already identifies the currency. */
export function formatUsdAmount(value: Amount): string {
  if (value == null || (typeof value === "string" && !value.trim())) return "—";
  const numeric = Number(value);
  if (!Number.isFinite(numeric)) return "—";
  if (numeric === 0) return "0";
  const format = new Intl.NumberFormat(currentLocale(), { maximumFractionDigits: 5 });
  // A ledger decimal just below 0.00001 may already round up to it as a JS number.
  const belowMinimum = typeof value === "string" && !/[eE]/.test(value)
    ? /^[+-]?0*\.0{5}/.test(value.trim())
    : Math.abs(numeric) < 0.00001;
  if (belowMinimum) {
    return `${numeric < 0 ? ">" : "<"}${format.format(numeric < 0 ? -0.00001 : 0.00001)}`;
  }
  // Pass decimal strings directly to Intl so grouping and rounding retain ledger precision.
  return format.format(value as number | Intl.StringNumericLiteral);
}

export function formatUsd(value: Amount): string {
  const amount = formatUsdAmount(value);
  return amount === "—" ? amount : `${amount} USD`;
}

/**
 * Infers a quota window's total allowance from its gateway-recorded spend and
 * the provider-reported percentage used. This is an estimate, not a
 * provider-supplied limit.
 */
export function formatEstimatedQuotaTotal(
  spendAmount: string | null | undefined,
  usedPercent: number | null | undefined,
): string {
  if (
    typeof spendAmount !== "string" ||
    spendAmount.trim() === "" ||
    typeof usedPercent !== "number" ||
    !Number.isFinite(usedPercent) ||
    usedPercent <= 0 ||
    usedPercent > 100
  ) {
    return "—";
  }

  const spend = Number(spendAmount);
  if (!Number.isFinite(spend) || spend < 0) return "—";

  const estimatedTotal = (spend * 100) / usedPercent;
  return Number.isFinite(estimatedTotal) && estimatedTotal >= 0
    ? formatUsd(String(estimatedTotal))
    : "—";
}

export function formatExactTokens(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return value.toLocaleString(currentLocale());
}

const tokenUnits = ["", "K", "M", "B"] as const;

export function formatTokens(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "—";
  let scaled = value;
  let unit = 0;
  // Promote before formatting a value that would round to 1,000 in this unit.
  while (unit < tokenUnits.length - 1 && Math.abs(scaled) >= 999.995) {
    scaled /= 1_000;
    unit++;
  }
  return scaled.toLocaleString(currentLocale(), { maximumFractionDigits: 2 }) + tokenUnits[unit];
}

export function formatBytes(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"] as const;
  let amount = Math.max(0, value);
  let unit = 0;
  while (amount >= 1_024 && unit < units.length - 1) {
    amount /= 1_024;
    unit += 1;
  }
  return `${amount.toLocaleString(currentLocale(), {
    maximumFractionDigits: amount >= 100 || unit === 0 ? 0 : 1,
  })} ${units[unit]}`;
}

export function formatDurationMs(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  if (value < 1000) return `${value} ms`;
  return `${(value / 1000).toLocaleString(currentLocale(), { maximumFractionDigits: 2 })} s`;
}

export function formatBoolean(value: boolean | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return value ? translate("Yes") : translate("No");
}

export function formatList(values: string[] | null | undefined, fallback = "—"): string {
  if (!values || values.length === 0) return fallback;
  return values.join(", ");
}

export function truncate(value: string, max = 48): string {
  if (value.length <= max) return value;
  return `${value.slice(0, max - 1)}…`;
}
