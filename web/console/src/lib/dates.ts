/** Lightweight timestamp primitives and display helpers (no date library). */

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
  if (!withTime) return `${year}-${month}-${day}`;
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
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return formatDate(value);
}

/** Expiry display with an explicit "never" when absent. */
export function formatExpiry(value: string | null | undefined): string {
  if (!value) return "never";
  return formatDateTime(value);
}
