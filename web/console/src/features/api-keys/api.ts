import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut, apiSend, buildQuery } from "@/api/client";
import type {
  ApiHostsView,
  ApiKeyPage,
  ApiKeyView,
  MutationResponse,
  OwnApiKeyListQuery,
  RevokeInput,
  SelfApiKeyCreateInput,
  SelfApiKeyOptions,
  SelfApiKeyUpdateInput,
} from "@/api/types";

const LIST_KEY = ["console", "me", "api-keys"] as const;
const API_HOSTS_KEY = ["console", "me", "api-hosts"] as const;

function detailKey(id: string) {
  return ["console", "me", "api-keys", id] as const;
}

export function useOwnApiKeys(
  params: OwnApiKeyListQuery = {},
  options?: { enabled?: boolean },
) {
  return useQuery({
    queryKey: [...LIST_KEY, params] as const,
    queryFn: ({ signal }) => apiGet<ApiKeyPage>(`/me/api-keys${buildQuery(params)}`, signal),
    placeholderData: keepPreviousData,
    enabled: options?.enabled ?? true,
  });
}

export function useOwnApiKeyOptions(enabled = true) {
  return useQuery({
    queryKey: ["console", "me", "api-key-options"] as const,
    queryFn: ({ signal }) => apiGet<SelfApiKeyOptions>("/me/api-key-options", signal),
    enabled,
  });
}

export function useApiHosts() {
  return useQuery({
    queryKey: API_HOSTS_KEY,
    queryFn: ({ signal }) => apiGet<ApiHostsView>("/me/api-hosts", signal),
  });
}

export function useOwnApiKey(id: string) {
  return useQuery({
    queryKey: detailKey(id),
    queryFn: ({ signal }) => apiGetDetail<ApiKeyView>(`/me/api-keys/${id}`, signal),
    enabled: Boolean(id),
    staleTime: 0,
    refetchOnWindowFocus: true,
  });
}

export function useCreateOwnApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: SelfApiKeyCreateInput) =>
      apiPost<MutationResponse>("/me/api-keys", input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: LIST_KEY });
    },
  });
}

export function useUpdateOwnApiKey(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: SelfApiKeyUpdateInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`/me/api-keys/${id}`, input, ifMatch),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: LIST_KEY }),
  });
}

export function useRevokeOwnApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, reason }: { id: string; reason: RevokeInput }) =>
      apiPost<MutationResponse>(`/me/api-keys/${id}/revoke`, reason),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: LIST_KEY });
    },
  });
}

export function useDeleteOwnApiKey(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/me/api-keys/${id}`, "DELETE", undefined, {
        ifMatch,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: LIST_KEY });
      queryClient.removeQueries({ queryKey: detailKey(id) });
    },
  });
}
