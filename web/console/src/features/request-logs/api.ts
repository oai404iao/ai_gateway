import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type { ListQuery, RequestLogView } from "@/api/types";

function ownListKey(limit: number) {
  return ["console", "me", "request-logs", limit] as const;
}

function allListKey(limit: number) {
  return ["console", "request-logs", limit] as const;
}

export function useOwnRequestLogs(limit: number) {
  return useQuery({
    queryKey: ownListKey(limit),
    queryFn: () => apiGet<RequestLogView[]>(`/me/request-logs?limit=${limit}`),
  });
}

export function useAllRequestLogs(limit: number) {
  return useQuery({
    queryKey: allListKey(limit),
    queryFn: () => apiGet<RequestLogView[]>(`/request-logs?limit=${limit}`),
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
