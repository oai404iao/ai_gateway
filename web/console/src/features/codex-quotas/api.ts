import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type {
  SelfCodexQuotaCredentialView,
  SelfCodexQuotaWindowHistory,
} from "@/api/types";

const CODEX_QUOTAS_KEY = ["console", "me", "codex-quotas"] as const;

export function useOwnCodexQuotas() {
  return useQuery({
    queryKey: CODEX_QUOTAS_KEY,
    queryFn: () =>
      apiGet<SelfCodexQuotaCredentialView[]>("/me/codex-quotas"),
  });
}

export function useOwnCodexQuotaWindowHistory(id: string) {
  return useQuery({
    queryKey: [...CODEX_QUOTAS_KEY, id, "windows"] as const,
    queryFn: () =>
      apiGet<SelfCodexQuotaWindowHistory>(
        `/me/codex-quotas/${id}/windows?limit=100`,
      ),
    enabled: Boolean(id),
  });
}
