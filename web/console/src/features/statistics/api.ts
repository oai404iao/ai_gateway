import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type {
  ChannelStatusReport,
  ChannelStatusWindow,
  CostStatisticsReport,
  PersonalUsageReport,
  StatisticsGranularity,
} from "@/api/types";

export interface CostStatisticsFilters {
  started_after: string;
  started_before: string;
  granularity: StatisticsGranularity;
  user_id?: string;
  api_key_id?: string;
  channel_id?: string;
}

export type CostStatisticsScope = "own" | "system";

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

export function usePersonalUsage() {
  return useQuery({
    queryKey: ["console", "statistics", "personal-usage"] as const,
    queryFn: () => apiGet<PersonalUsageReport>("/me/usage"),
  });
}

export function useCostStatistics(
  scope: CostStatisticsScope,
  filters: CostStatisticsFilters,
) {
  const systemScope = scope === "system";
  return useQuery({
    queryKey: ["console", scope, "statistics", "costs", filters] as const,
    queryFn: () =>
      apiGet<CostStatisticsReport>(
        `${systemScope ? "/system/statistics/costs" : "/statistics/costs"}?${queryString({
          started_after: filters.started_after,
          started_before: filters.started_before,
          granularity: filters.granularity,
          user_id: systemScope ? filters.user_id : undefined,
          api_key_id: filters.api_key_id,
          channel_id: systemScope ? filters.channel_id : undefined,
        })}`,
      ),
  });
}
