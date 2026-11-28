import type { StatisticsGranularity } from "@/api/types";
import type { CostStatisticsFilters, CostStatisticsScope } from "@/features/statistics/api";
import { dateTimeLocalToIso, formatDateTimeLocalInput } from "@/lib/dates";
import { overviewWindow } from "./usage-window";

export interface CostFilterDraft {
  started_after: string;
  started_before: string;
  granularity: StatisticsGranularity;
  user_id: string;
  api_key_id: string;
  channel_id: string;
}

export type QuickRange = "today" | "this_week" | "this_month";

export const CHART_COLORS = [1, 3, 4, 5].map((i) => `var(--chart-${i})`);

export function defaultDraft(): CostFilterDraft {
  const endedAt = new Date();
  endedAt.setSeconds(0, 0);
  const startedAt = new Date(endedAt);
  startedAt.setHours(0, 0, 0, 0);
  return {
    started_after: formatDateTimeLocalInput(startedAt.toISOString(), true),
    started_before: formatDateTimeLocalInput(endedAt.toISOString(), true),
    granularity: "hour",
    user_id: "",
    api_key_id: "",
    channel_id: "",
  };
}

export function draftFromSearchParams(
  search: URLSearchParams,
  scope: CostStatisticsScope,
): CostFilterDraft {
  const draft = defaultDraft();
  if (scope === "own" && !search.has("started_after") && !search.has("started_before")) {
    const bounds = overviewWindow(Math.floor(Date.now() / 1000) * 1000, "7d");
    draft.started_after = formatDateTimeLocalInput(bounds.started_after, true);
    draft.started_before = formatDateTimeLocalInput(bounds.started_before, true);
  }
  const startedAfter = search.get("started_after");
  const startedBefore = search.get("started_before");
  const granularity = search.get("granularity");
  if (startedAfter && formatDateTimeLocalInput(startedAfter, true)) {
    draft.started_after = formatDateTimeLocalInput(startedAfter, true);
  }
  if (startedBefore && formatDateTimeLocalInput(startedBefore, true)) {
    draft.started_before = formatDateTimeLocalInput(startedBefore, true);
  }
  if (granularity === "hour" || granularity === "day") {
    draft.granularity = granularity;
  }
  draft.api_key_id = search.get("api_key_id") ?? "";
  if (scope === "system") {
    draft.user_id = search.get("user_id") ?? "";
    draft.channel_id = search.get("channel_id") ?? "";
  }
  return draft;
}

export function quickRangeDraft(
  range: QuickRange,
  current: CostFilterDraft,
  now = new Date(),
): CostFilterDraft {
  const endedAt = new Date(now);
  endedAt.setSeconds(0, 0);
  const startedAt = new Date(endedAt);
  if (range === "today") {
    startedAt.setHours(0, 0, 0, 0);
  } else if (range === "this_week") {
    const daysSinceMonday = (startedAt.getDay() + 6) % 7;
    startedAt.setDate(startedAt.getDate() - daysSinceMonday);
    startedAt.setHours(0, 0, 0, 0);
  } else {
    startedAt.setDate(1);
    startedAt.setHours(0, 0, 0, 0);
  }
  return {
    ...current,
    started_after: formatDateTimeLocalInput(startedAt.toISOString(), true),
    started_before: formatDateTimeLocalInput(endedAt.toISOString(), true),
    granularity: range === "today" ? "hour" : "day",
  };
}

export function toFilters(draft: CostFilterDraft): CostStatisticsFilters | null {
  const startedAfter = dateTimeLocalToIso(draft.started_after);
  const startedBefore = dateTimeLocalToIso(draft.started_before);
  if (!startedAfter || !startedBefore) return null;
  return {
    started_after: startedAfter,
    started_before: startedBefore,
    granularity: draft.granularity,
    user_id: draft.user_id || undefined,
    api_key_id: draft.api_key_id || undefined,
    channel_id: draft.channel_id || undefined,
  };
}

export function sameCostStatisticsFilters(
  left: CostStatisticsFilters,
  right: CostStatisticsFilters,
): boolean {
  return (
    left.started_after === right.started_after &&
    left.started_before === right.started_before &&
    left.granularity === right.granularity &&
    left.user_id === right.user_id &&
    left.api_key_id === right.api_key_id &&
    left.channel_id === right.channel_id
  );
}

export function searchParamsForFilters(
  filters: CostStatisticsFilters,
  scope: CostStatisticsScope,
): URLSearchParams {
  const search = new URLSearchParams({
    started_after: filters.started_after,
    started_before: filters.started_before,
    granularity: filters.granularity,
  });
  if (filters.api_key_id) search.set("api_key_id", filters.api_key_id);
  if (scope === "system") {
    if (filters.user_id) search.set("user_id", filters.user_id);
    if (filters.channel_id) search.set("channel_id", filters.channel_id);
  }
  return search;
}

export function numericAmount(value: string): number {
  const amount = Number(value);
  return Number.isFinite(amount) ? amount : 0;
}

export function formatRate(value: number | null): string {
  if (value === null) return "—";
  return `${(value * 100).toLocaleString(undefined, { maximumFractionDigits: 1 })}%`;
}

export function cacheRate(inputTokens: number, cachedInputTokens: number): number | null {
  return inputTokens > 0 ? cachedInputTokens / inputTokens : null;
}

