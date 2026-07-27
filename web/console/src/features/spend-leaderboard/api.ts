import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type {
  SpendLeaderboardPeriod,
  SpendLeaderboardReport,
} from "@/api/types";

export interface SpendLeaderboardFilters {
  period: SpendLeaderboardPeriod;
  period_start?: string;
  limit?: number;
}

function queryString(values: Record<string, string | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value) search.set(key, value);
  }
  return search.toString();
}

export function useSpendLeaderboard(filters: SpendLeaderboardFilters) {
  return useQuery({
    queryKey: ["console", "spend-leaderboard", filters] as const,
    queryFn: () =>
      apiGet<SpendLeaderboardReport>(
        `/statistics/spend-leaderboard?${queryString({
          period: filters.period,
          period_start: filters.period_start,
          limit: String(filters.limit ?? 50),
        })}`,
      ),
  });
}
