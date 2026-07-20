import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiSend } from "@/api/client";
import { clearSession, getSession, setSession } from "@/api/session-store";
import type { ConsoleProfile, PasswordChangeInput, ProfileUpdateInput } from "@/api/types";

const PROFILE_KEY = ["console", "me", "profile"] as const;

export function useProfile() {
  return useQuery({
    queryKey: PROFILE_KEY,
    queryFn: () => apiGet<ConsoleProfile>("/me"),
  });
}

export function useUpdateProfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: ProfileUpdateInput) => apiSend<ConsoleProfile>("/me", "PATCH", input),
    onSuccess: (profile) => {
      queryClient.setQueryData(PROFILE_KEY, profile);
      const session = getSession();
      if (session.user) {
        setSession({
          user: {
            ...session.user,
            display_name: profile.display_name,
          },
        });
      }
    },
  });
}

export function useChangePassword() {
  return useMutation({
    mutationFn: (input: PasswordChangeInput) => apiSend<void>("/me/password", "POST", input),
    onSuccess: () => {
      clearSession();
    },
  });
}
