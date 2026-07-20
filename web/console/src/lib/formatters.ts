/** Formatting helpers for Console display values. Decimals arrive as strings. */

import { currentLocale, translate } from "@/app/i18n";

export function formatDecimal(value: string | null | undefined, fractionDigits = 6): string {
  if (value === null || value === undefined || value === "") return "—";
  const number = Number(value);
  if (!Number.isFinite(number)) return value;
  return number.toLocaleString(currentLocale(), {
    minimumFractionDigits: 0,
    maximumFractionDigits: fractionDigits,
  });
}

export function formatCurrency(value: string | null | undefined, currency?: string | null): string {
  if (value === null || value === undefined || value === "") return "—";
  const number = Number(value);
  const amount = Number.isFinite(number)
    ? number.toLocaleString(currentLocale(), {
        minimumFractionDigits: 2,
        maximumFractionDigits: 6,
      })
    : value;
  return currency ? `${amount} ${currency}` : amount;
}

export function formatTokens(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return value.toLocaleString(currentLocale());
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
