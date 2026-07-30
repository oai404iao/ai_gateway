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
  MutationResponse,
} from "@/api/types";

const credentialsKey = (groupId: string) =>
  ["console", "codex-oauth", groupId, "credentials"] as const;
const credentialKey = (id: string) =>
  ["console", "codex-oauth", "credential", id] as const;

export function useCodexCredentials(groupId: string) {
  return useQuery({
    queryKey: credentialsKey(groupId),
    queryFn: () =>
      apiGet<CodexCredentialView[]>(
        `/providers/codex-oauth/channel-groups/${groupId}/credentials`,
      ),
    enabled: Boolean(groupId),
    refetchInterval: 30_000,
  });
}

export function useCodexCredential(id: string) {
  const query = useQuery({
    queryKey: credentialKey(id),
    queryFn: () =>
      apiGetDetail<CodexCredentialView>(
        `/providers/codex-oauth/credentials/${id}`,
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

export function useStartCodexOauth(groupId: string) {
  return useMutation({
    mutationFn: (input: CodexOauthStartInput) =>
      apiPost<CodexOauthStartResponse>(
        `/providers/codex-oauth/channel-groups/${groupId}/oauth/flows`,
        input,
      ),
  });
}

export function useCompleteCodexOauth(groupId: string) {
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
        `/providers/codex-oauth/oauth/flows/${flowId}/complete`,
        input,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useImportCodexCredential(groupId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CodexCredentialImportInput) =>
      apiPost<MutationResponse>(
        `/providers/codex-oauth/channel-groups/${groupId}/credentials`,
        input,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useExportCodexCredentials(groupId: string) {
  return useMutation({
    mutationFn: (input: CodexCredentialExportInput) =>
      apiPost<CodexCredentialExportBundle>(
        `/providers/codex-oauth/channel-groups/${groupId}/credentials/export`,
        input,
      ),
  });
}

export function useUpdateCodexCredential(groupId: string, id: string) {
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
        `/providers/codex-oauth/credentials/${id}`,
        input,
        ifMatch,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useDeleteCodexCredential(groupId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ifMatch }: { id: string; ifMatch: string }) =>
      apiSend<MutationResponse>(
        `/providers/codex-oauth/credentials/${id}`,
        "DELETE",
        undefined,
        { ifMatch },
      ),
    onSuccess: (_data, { id }) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.removeQueries({ queryKey: credentialKey(id) });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useBatchUpdateCodexCredentials(groupId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CodexCredentialBatchInput) =>
      apiPost<CodexCredentialBatchResponse>(
        `/providers/codex-oauth/channel-groups/${groupId}/credentials/batch`,
        input,
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.invalidateQueries({
        queryKey: ["console", "codex-oauth", "credential"],
      });
      void queryClient.invalidateQueries({ queryKey: ["console", "channels"] });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

export function useRefreshCodexCredential(groupId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiPost<void>(`/providers/codex-oauth/credentials/${id}/refresh`),
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
    },
  });
}

export function useRefreshCodexQuota(groupId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiPost<void>(
        `/providers/codex-oauth/credentials/${id}/quota/refresh`,
      ),
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: credentialsKey(groupId) });
      void queryClient.invalidateQueries({ queryKey: credentialKey(id) });
    },
  });
}
