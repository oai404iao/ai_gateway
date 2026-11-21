import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiDelete, apiGet } from "@/api/client";
import { clearSession, getSession } from "@/api/session-store";
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
    mutationFn: async ({ id, isCurrent }: { id: string; isCurrent: boolean }) => {
      const ownerId = getSession().user?.id;
      await apiDelete(`/me/sessions/${id}`);
      if (isCurrent && ownerId && getSession().user?.id === ownerId) clearSession();
    },
    onSuccess: (_data, variables) => {
      if (!variables.isCurrent) {
        void queryClient.invalidateQueries({ queryKey: SESSIONS_KEY });
      }
    },
  });
}

export function useRevokeOtherSessions() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => apiDelete("/me/sessions"),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SESSIONS_KEY });
    },
  });
}
