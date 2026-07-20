import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type { ListQuery, RequestLogView } from "@/api/types";

function ownListKey(filters: ListQuery) {
  return ["console", "me", "request-logs", filters] as const;
}

function allListKey(filters: ListQuery) {
  return ["console", "request-logs", filters] as const;
}

function requestLogQuery(filters: ListQuery): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) {
    if (value !== undefined && value !== null && value !== "") {
      search.set(key, String(value));
    }
  }
  const query = search.toString();
  return query ? `?${query}` : "";
}

export function useOwnRequestLogs(filters: ListQuery) {
  return useQuery({
    queryKey: ownListKey(filters),
    queryFn: () => apiGet<RequestLogView[]>(`/me/request-logs${requestLogQuery(filters)}`),
  });
}

export function useAllRequestLogs(filters: ListQuery) {
  return useQuery({
    queryKey: allListKey(filters),
    queryFn: () => apiGet<RequestLogView[]>(`/request-logs${requestLogQuery(filters)}`),
  });
}

export function useRequestLog(basePath: string, id: string | null) {
  return useQuery({
    queryKey: [basePath, id] as const,
    queryFn: () => apiGet<RequestLogView>(`${basePath}/${id}`),
    enabled: Boolean(id),
  });
}

export type { ListQuery };
