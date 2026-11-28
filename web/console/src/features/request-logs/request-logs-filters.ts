import type {
  AdminRequestLogQuery,
  RequestLogPage,
} from "@/api/types";
import { dateTimeLocalToIso, formatDateTimeLocalInput } from "@/lib/dates";
import { API_FORMATS, API_OPERATIONS } from "@/lib/permissions";

export const LIMITS = [25, 50, 100] as const;
export const OUTCOMES = ["succeeded", "failed", "rejected", "cancelled"] as const;
export const uuid = /^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i;

export type ApiFormatFilter = NonNullable<AdminRequestLogQuery["api_format"]>;
export type ApiOperationFilter = NonNullable<AdminRequestLogQuery["api_operation"]>;
export type OutcomeFilter = NonNullable<AdminRequestLogQuery["outcome"]>;

export interface RequestLogListResult {
  data: RequestLogPage | undefined;
  isLoading: boolean;
  isFetching: boolean;
  error: unknown;
  refetch: () => Promise<unknown>;
}

export type UseRequestLogs = (filters: AdminRequestLogQuery) => RequestLogListResult;

export interface RequestLogFilterDraft {
  limit: (typeof LIMITS)[number];
  user_id: string;
  api_key_id: string;
  model: string;
  api_format: "" | ApiFormatFilter;
  api_operation: "" | ApiOperationFilter;
  outcome: "" | OutcomeFilter;
  started_after: string;
  started_before: string;
  billed: "" | "true" | "false";
}

export const emptyFilters: RequestLogFilterDraft = {
  limit: 50,
  user_id: "",
  api_key_id: "",
  model: "",
  api_format: "",
  api_operation: "",
  outcome: "",
  started_after: "",
  started_before: "",
  billed: "",
};

export const REQUEST_LOG_FILTER_KEYS = [
  "limit",
  "user_id",
  "api_key_id",
  "model",
  "api_format",
  "api_operation",
  "outcome",
  "started_after",
  "started_before",
  "billed",
] as const satisfies readonly (keyof AdminRequestLogQuery)[];

export function readFilters(search: string, system: boolean): AdminRequestLogQuery {
  const params = new URLSearchParams(search);
  const identifier = (key: string) => {
    const value = params.get(key);
    return value && uuid.test(value) ? value : undefined;
  };
  const instant = (key: string) => {
    const value = params.get(key);
    if (!value || !/(?:Z|[+-]\d\d:\d\d)$/i.test(value) || !Number.isFinite(Date.parse(value))) return undefined;
    return value;
  };
  return {
    limit: LIMITS.find(v => String(v) === params.get("limit")) ?? 50,
    user_id: system ? identifier("user_id") : undefined, api_key_id: identifier("api_key_id"),
    model: params.get("model")?.trim() || undefined,
    api_format: API_FORMATS.find(v => v === params.get("api_format")),
    api_operation: API_OPERATIONS.find(v => v === params.get("api_operation")),
    outcome: OUTCOMES.find(v => v === params.get("outcome")),
    started_after: instant("started_after"), started_before: instant("started_before"),
    billed: params.get("billed") === "true" ? true : params.get("billed") === "false" ? false : undefined,
  };
}

export function filterDraft(query: AdminRequestLogQuery): RequestLogFilterDraft {
  return {
    limit: LIMITS.find(value => value === query.limit) ?? 50,
    user_id: query.user_id ?? "",
    api_key_id: query.api_key_id ?? "",
    model: query.model ?? "",
    api_format: query.api_format ?? "",
    api_operation: query.api_operation ?? "",
    outcome: query.outcome ?? "",
    started_after: formatDateTimeLocalInput(query.started_after, true),
    started_before: formatDateTimeLocalInput(query.started_before, true),
    billed: query.billed === undefined ? "" : query.billed ? "true" : "false",
  };
}

export interface RequestLogsViewProps {
  title: string;
  basePath: string;
  useLogs: UseRequestLogs;
  scope: "own" | "system";
}

export function optionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed || undefined;
}


export function toQuery(draft: RequestLogFilterDraft): AdminRequestLogQuery {
  return {
    limit: draft.limit,
    user_id: optionalText(draft.user_id),
    api_key_id: optionalText(draft.api_key_id),
    model: optionalText(draft.model),
    api_format: draft.api_format || undefined,
    api_operation: draft.api_operation || undefined,
    outcome: draft.outcome || undefined,
    started_after: dateTimeLocalToIso(draft.started_after) ?? undefined,
    started_before: dateTimeLocalToIso(draft.started_before) ?? undefined,
    billed: draft.billed === "" ? undefined : draft.billed === "true",
  };
}
