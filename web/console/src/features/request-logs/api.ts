import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { apiGet, buildQuery } from "@/api/client";
import type { paths } from "@/api/generated/console-v1";
import type {
  AdminRequestLogQuery,
  OwnRequestLogQuery,
  RequestLogPage,
  RequestLogView,
} from "@/api/types";

function ownListKey(filters: OwnRequestLogQuery) {
  return ["console", "me", "request-logs", filters] as const;
}

function allListKey(filters: AdminRequestLogQuery) {
  return ["console", "request-logs", filters] as const;
}

export function useOwnRequestLogs(filters: OwnRequestLogQuery) {
  return useQuery({
    queryKey: ownListKey(filters),
    queryFn: ({ signal }) => apiGet<RequestLogPage>(`/me/request-logs${buildQuery(filters)}`, signal),
    placeholderData: keepPreviousData,
  });
}

export function useAllRequestLogs(filters: AdminRequestLogQuery) {
  return useQuery({
    queryKey: allListKey(filters),
    queryFn: ({ signal }) => apiGet<RequestLogPage>(`/request-logs${buildQuery(filters)}`, signal),
    placeholderData: keepPreviousData,
  });
}

type ModelOptions = paths["/model-options"]["get"]["responses"][200]["content"]["application/json"];

export function useModelOptions(enabled: boolean) {
  return useQuery({
    queryKey: ["console", "model-options"],
    queryFn: ({ signal }) => apiGet<ModelOptions>("/model-options", signal),
    enabled,
  });
}

export function useRequestLog(basePath: string, id: string | null) {
  return useQuery({
    queryKey: ["console", basePath, id] as const,
    queryFn: ({ signal }) => apiGet<RequestLogView>(`${basePath}/${id}`, signal),
    enabled: Boolean(id),
  });
}
