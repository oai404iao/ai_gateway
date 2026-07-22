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

export function formatUsd(value: string | null | undefined): string {
  if (value === null || value === undefined || value === "") return "—";
  const number = Number(value);
  const amount = Number.isFinite(number)
    ? number.toLocaleString(currentLocale(), {
        minimumFractionDigits: 2,
        maximumFractionDigits: 6,
      })
    : value;
  return `${amount} USD`;
}

export function formatTokens(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return value.toLocaleString(currentLocale());
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
