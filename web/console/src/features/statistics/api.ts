import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type {
  ChannelStatusReport,
  ChannelStatusWindow,
  CostStatisticsReport,
  StatisticsGranularity,
  SystemLoadReport,
} from "@/api/types";

export interface CostStatisticsFilters {
  started_after: string;
  started_before: string;
  granularity: StatisticsGranularity;
  user_id?: string;
  api_key_id?: string;
}

function queryString(values: Record<string, string | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value) search.set(key, value);
  }
  return search.toString();
}

export function useChannelStatus(window: ChannelStatusWindow) {
  return useQuery({
    queryKey: ["console", "statistics", "channel-status", window] as const,
    queryFn: () =>
      apiGet<ChannelStatusReport>(
        `/statistics/channel-status?${queryString({ window })}`,
      ),
  });
}

export function useCostStatistics(filters: CostStatisticsFilters) {
  return useQuery({
    queryKey: ["console", "statistics", "costs", filters] as const,
    queryFn: () =>
      apiGet<CostStatisticsReport>(
        `/statistics/costs?${queryString({
          started_after: filters.started_after,
          started_before: filters.started_before,
          granularity: filters.granularity,
          user_id: filters.user_id,
          api_key_id: filters.api_key_id,
        })}`,
      ),
  });
}

export function useSystemLoad() {
  return useQuery({
    queryKey: ["console", "system", "load"] as const,
    queryFn: () => apiGet<SystemLoadReport>("/system/load"),
    refetchInterval: 5_000,
  });
}
