import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut, apiSend } from "@/api/client";
import type {
  CodexCredentialBatchInput,
  CodexCredentialBatchResponse,
  CodexCredentialExportBundle,
  CodexCredentialExportInput,
  CodexCredentialImportInput,
  CodexCredentialUpdateInput,
  CodexCredentialView,
  CodexOauthCompleteInput,
  CodexOauthStartInput,
  CodexOauthStartResponse,
  CodexQuotaResetResponse,
  CodexQuotaWindowHistory,
  MutationResponse,
} from "@/api/types";

const credentialsKey = ["console", "codex-oauth", "credentials"] as const;
const credentialKey = (id: string) =>
  ["console", "codex-oauth", "credential", id] as const;
const quotaWindowHistoryKey = (id: string) =>
  ["console", "codex-oauth", "credential", id, "quota-windows"] as const;

export function useCodexCredentials() {
  return useQuery({
    queryKey: credentialsKey,
    queryFn: () =>
      apiGet<CodexCredentialView[]>(
        "/routing/upstream-credentials/codex",
      ),
    refetchInterval: 30_000,
  });
}

export function useCodexCredential(id: string) {
  const query = useQuery({
    queryKey: credentialKey(id),
    queryFn: () =>
      apiGetDetail<CodexCredentialView>(
        `/routing/upstream-credentials/codex/${id}`,
      ),
    enabled: Boolean(id),
  });
  return {
    data: query.data,
    etag: query.data?.etag ?? "",
    isLoading: query.isLoading,
    error: query.error,
    refetch: query.refetch,
  };
}

export function useCodexQuotaWindowHistory(id: string) {
  return useQuery({
    queryKey: quotaWindowHistoryKey(id),
    queryFn: () =>
      apiGet<CodexQuotaWindowHistory>(
        `/routing/upstream-credentials/codex/${id}/quota/windows`,
      ),
    enabled: Boolean(id),
  });
}

export function useStartCodexOauth() {
  return useMutation({
    mutationFn: (input: CodexOauthStartInput) =>
      apiPost<CodexOauthStartResponse>(
        "/routing/upstream-credentials/codex/oauth/flows",
        input,
      ),
  });
}

export function useCompleteCodexOauth() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      flowId,
      input,
    }: {
      flowId: string;
      input: CodexOauthCompleteInput;
    }) =>
      apiPost<MutationResponse>(
        `/routing/upstream-credentials/codex/oauth/flows/${flowId}/complete`,
        input,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: ["console", "upstream-credentials"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "model-rules"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useImportCodexCredential() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CodexCredentialImportInput) =>
      apiPost<MutationResponse>(
        "/routing/upstream-credentials/codex",
        input,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: ["console", "upstream-credentials"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "model-rules"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useExportCodexCredentials() {
  return useMutation({
    mutationFn: (input: CodexCredentialExportInput) =>
      apiPost<CodexCredentialExportBundle>(
        "/routing/upstream-credentials/codex/export",
        input,
      ),
  });
}

export function useUpdateCodexCredential(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      input,
      ifMatch,
    }: {
      input: CodexCredentialUpdateInput;
      ifMatch: string;
    }) =>
      apiPut<MutationResponse>(
        `/routing/upstream-credentials/codex/${id}`,
        input,
        ifMatch,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: ["console", "upstream-credentials"] });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "model-rules"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useDeleteCodexCredential() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ifMatch }: { id: string; ifMatch: string }) =>
      apiSend<MutationResponse>(
        `/routing/upstream-credentials/codex/${id}`,
        "DELETE",
        undefined,
        { ifMatch },
      ),
    onSuccess: (_data, { id }) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: ["console", "upstream-credentials"] });
      void queryClient.removeQueries({ queryKey: credentialKey(id) });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "model-rules"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useBatchUpdateCodexCredentials() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CodexCredentialBatchInput) =>
      apiPost<CodexCredentialBatchResponse>(
        "/routing/upstream-credentials/codex/batch",
        input,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: ["console", "upstream-credentials"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "codex-oauth", "credential"],
      });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({ queryKey: ["console", "model-rules"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useRefreshCodexCredential() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiPost<void>(`/routing/upstream-credentials/codex/${id}/refresh`),
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
    },
  });
}

export function useRefreshCodexQuota() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiPost<void>(
        `/routing/upstream-credentials/codex/${id}/quota/refresh`,
      ),
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
    },
  });
}

export function useResetCodexQuota() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiPost<CodexQuotaResetResponse>(
        `/routing/upstream-credentials/codex/${id}/quota/reset`,
      ),
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
      void queryClient.invalidateQueries({
        queryKey: quotaWindowHistoryKey(id),
      });
    },
  });
}
