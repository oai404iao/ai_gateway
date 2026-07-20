import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiDelete, apiGet } from "@/api/client";
import type { ConsoleSession } from "@/api/types";

const SESSIONS_KEY = ["console", "me", "sessions"] as const;

export function useSessions() {
  return useQuery({
    queryKey: SESSIONS_KEY,
    queryFn: () => apiGet<ConsoleSession[]>("/me/sessions"),
  });
}

export function useRevokeSession() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/me/sessions/${id}`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SESSIONS_KEY });
    },
  });
}
