import type { CostStatisticsBucketModel, CostStatisticsReport } from "@/api/types";
import type { CostStatisticsFilters } from "./api";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
export type UsagePeriod = "24h" | "7d";
export type UsageMetric = "request_count" | "total_tokens" | "cost_amount";
export type UsageBounds = Pick<CostStatisticsFilters, "started_after" | "started_before">;

export function overviewWindow(end: number, period: UsagePeriod, previous = false): UsageBounds {
  const shift = previous ? (period === "7d" ? 7 : 1) * DAY : 0;
  const start = period === "24h" ? end - DAY : Math.floor(end / DAY) * DAY - 6 * DAY;
  return {
    started_after: new Date(start - shift).toISOString(),
    started_before: new Date(end - shift).toISOString(),
  };
}

export function requestLogBounds(bounds: UsageBounds): UsageBounds {
  // Statistics intervals are half-open; the request-log API includes its upper bound.
  const lastMs = new Date(Date.parse(bounds.started_before) - 1).toISOString();
  return { started_after: bounds.started_after, started_before: lastMs.replace("Z", "999Z") };
}

export function ownLogsLink(bounds: UsageBounds, filters: Record<string, string> = {}) {
  return `/usage/request-logs?${new URLSearchParams({ ...requestLogBounds(bounds), ...filters })}`;
}

export function usageSlices(report: CostStatisticsReport, groupByDay: boolean) {
  const step = report.granularity === "day" ? DAY : HOUR;
  const slices: {
    start: number;
    end: number;
    first: string;
    last: string;
    values: Record<UsageMetric, number>;
  }[] = [];
  const start = Date.parse(report.started_at),
    end = Date.parse(report.ended_at);
  for (const bucket of report.buckets) {
    const time = Date.parse(bucket.started_at),
      clippedStart = Math.max(start, time),
      clippedEnd = Math.min(end, time + step);
    if (clippedEnd <= clippedStart) continue;
    let slice = slices.at(-1);
    if (!slice || !groupByDay || Math.floor(slice.start / DAY) !== Math.floor(time / DAY)) {
      slice = {
        start: clippedStart,
        end: clippedEnd,
        first: bucket.started_at,
        last: bucket.started_at,
        values: { request_count: 0, total_tokens: 0, cost_amount: 0 },
      };
      slices.push(slice);
    }
    slice.end = clippedEnd;
    slice.last = bucket.started_at;
    for (const key of ["request_count", "total_tokens", "cost_amount"] as const)
      slice.values[key] += Number(bucket[key]);
  }
  return slices;
}

export function modelIdentity(model: CostStatisticsBucketModel) {
  return `${model.api_format}:${model.model}`;
}

export function byRequestVolume(a: CostStatisticsBucketModel, b: CostStatisticsBucketModel) {
  return b.request_count - a.request_count || modelIdentity(a).localeCompare(modelIdentity(b));
}
