import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut } from "@/api/client";
import type {
  ApiKeyView,
  MutationResponse,
  RevokeInput,
  SelfApiKeyCreateInput,
  SelfApiKeyUpdateInput,
} from "@/api/types";

const LIST_KEY = ["console", "me", "api-keys"] as const;

function detailKey(id: string) {
  return ["console", "me", "api-keys", id] as const;
}

export function useOwnApiKeys() {
  return useQuery({
    queryKey: LIST_KEY,
    queryFn: () => apiGet<ApiKeyView[]>("/me/api-keys"),
  });
}

export function useOwnApiKey(id: string) {
  const query = useQuery({
    queryKey: detailKey(id),
    queryFn: () => apiGetDetail<ApiKeyView>(`/me/api-keys/${id}`),
  });
  return {
    data: query.data,
    etag: query.data?.etag ?? "",
    isLoading: query.isLoading,
    error: query.error,
  };
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
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: LIST_KEY });
      void queryClient.invalidateQueries({ queryKey: detailKey(id) });
    },
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
