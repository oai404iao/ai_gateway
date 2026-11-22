import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type { paths } from "@/api/generated/console-v1";
import type { AdminRequestLogQuery, OwnRequestLogQuery, RequestLogView } from "@/api/types";

function ownListKey(filters: OwnRequestLogQuery) {
  return ["console", "me", "request-logs", filters] as const;
}

function allListKey(filters: AdminRequestLogQuery) {
  return ["console", "request-logs", filters] as const;
}

function requestLogQuery(filters: OwnRequestLogQuery | AdminRequestLogQuery): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) {
    if (value !== undefined && value !== null && value !== "") {
      search.set(key, String(value));
    }
  }
  const query = search.toString();
  return query ? `?${query}` : "";
}

export function useOwnRequestLogs(filters: OwnRequestLogQuery) {
  return useQuery({
    queryKey: ownListKey(filters),
    queryFn: () => apiGet<RequestLogView[]>(`/me/request-logs${requestLogQuery(filters)}`),
  });
}

export function useAllRequestLogs(filters: AdminRequestLogQuery) {
  return useQuery({
    queryKey: allListKey(filters),
    queryFn: () => apiGet<RequestLogView[]>(`/request-logs${requestLogQuery(filters)}`),
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
    queryKey: [basePath, id] as const,
    queryFn: () => apiGet<RequestLogView>(`${basePath}/${id}`),
    enabled: Boolean(id),
  });
}
