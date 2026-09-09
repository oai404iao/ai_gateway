import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut } from "@/api/client";
import type {
  CodexSharingGroup, CodexSharingGroupInput, CodexSharingGroupsResponse,
  CodexSharingSeatsResponse, MutationResponse, SelfCodexSharingView,
} from "@/api/types";

const key = ["console", "codex-sharing"] as const;
export function useSharingGroups() {
  return useQuery({ queryKey: key, queryFn: () => apiGet<CodexSharingGroupsResponse>("/codex-sharing-groups") });
}
export function useSharingGroup(id: string) {
  return useQuery({
    queryKey: [...key, id],
    queryFn: () => apiGetDetail<CodexSharingGroup>(`/codex-sharing-groups/${id}`),
    enabled: Boolean(id) && id !== "new",
  });
}
export function useSharingSeats(id: string) {
  return useQuery({
    queryKey: [...key, id, "usage"],
    queryFn: () => apiGet<CodexSharingSeatsResponse>(`/codex-sharing-groups/${id}/usage`),
    enabled: Boolean(id) && id !== "new", refetchInterval: 15_000,
  });
}
export function useSaveSharing(id: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ input, etag }: { input: CodexSharingGroupInput; etag?: string }) =>
      id === "new"
        ? apiPost<MutationResponse>("/codex-sharing-groups", input)
        : apiPut<MutationResponse>(`/codex-sharing-groups/${id}`, input, etag ?? ""),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: key });
      void client.invalidateQueries({ queryKey: ["console", "me", "codex-sharing"] });
    },
  });
}
export function useOwnSharing() {
  return useQuery({
    queryKey: ["console", "me", "codex-sharing"],
    queryFn: () => apiGet<SelfCodexSharingView>("/me/codex-sharing"), refetchInterval: 15_000,
  });
}
