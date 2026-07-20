/** Lightweight timestamp primitives and display helpers (no date library). */

import { currentLocale, translate } from "@/app/i18n";

function parseISO(value: string): Date | null {
  // Accept "Z" or explicit offset; reject anything that isn't a valid Date.
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

/** Local YYYY-MM-DD HH:MM using the browser's locale-independent formatting. */
function formatLocal(date: Date, withTime: boolean): string {
  const year = date.getFullYear();
  const month = pad(date.getMonth() + 1);
  const day = pad(date.getDate());
  if (!withTime) {
    return currentLocale() === "zh-CN" ? `${year}年${month}月${day}日` : `${year}-${month}-${day}`;
  }
  if (currentLocale() === "zh-CN") {
    return `${year}年${month}月${day}日 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
  }
  return `${year}-${month}-${day} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Human-readable absolute timestamp, e.g. "2026-01-02 03:04". */
export function formatDateTime(value: string | null | undefined): string {
  if (!value) return "—";
  const date = parseISO(value);
  if (!date) return value;
  return formatLocal(date, true);
}

/** Date-only timestamp, e.g. "2026-01-02". */
export function formatDate(value: string | null | undefined): string {
  if (!value) return "—";
  const date = parseISO(value);
  if (!date) return value;
  return formatLocal(date, false);
}

/** RFC 3339 timestamp formatted for an HTML `datetime-local` input. */
export function formatDateTimeLocalInput(value: string | null | undefined): string {
  if (!value) return "";
  const date = parseISO(value);
  if (!date) return "";
  // `datetime-local` accepts only an ISO-like numeric value, irrespective of
  // the surrounding UI locale.
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

/** Browser-local `datetime-local` value converted to an RFC 3339 UTC timestamp. */
export function dateTimeLocalToIso(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

/** Whether an optional browser-local timestamp is valid and still in the future. */
export function isFutureDateTimeLocal(value: string | null | undefined): boolean {
  if (!value) return true;
  const date = new Date(value);
  return !Number.isNaN(date.getTime()) && date.getTime() > Date.now();
}

export function differenceInMinutes(from: Date, to: Date): number {
  return Math.floor((from.getTime() - to.getTime()) / 60_000);
}

/** Relative time for sessions/audit, e.g. "3m ago". */
export function formatRelative(value: string | null | undefined): string {
  if (!value) return "—";
  const date = parseISO(value);
  if (!date) return value;
  const minutes = differenceInMinutes(new Date(), date);
  if (minutes < 0) return formatDateTime(value);
  if (minutes < 1) return translate("just now");
  if (minutes < 60) return translate("{minutes}m ago", { minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return translate("{hours}h ago", { hours });
  const days = Math.floor(hours / 24);
  if (days < 30) return translate("{days}d ago", { days });
  return formatDate(value);
}

/** Expiry display with an explicit "never" when absent. */
export function formatExpiry(value: string | null | undefined): string {
  if (!value) return translate("never");
  return formatDateTime(value);
}
