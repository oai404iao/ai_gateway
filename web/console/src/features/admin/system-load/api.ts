import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type { SystemLoadReport } from "@/api/types";

export function useSystemLoad() {
  return useQuery({
    queryKey: ["console", "system", "load"] as const,
    queryFn: () => apiGet<SystemLoadReport>("/system/load"),
    refetchInterval: 5_000,
  });
}
