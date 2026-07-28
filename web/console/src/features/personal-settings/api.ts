import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiSend } from "@/api/client";
import type { UserSettings, UserSettingsInput } from "@/api/types";

const USER_SETTINGS_KEY = ["console", "me", "settings"] as const;

export function useUserSettings() {
  return useQuery({
    queryKey: USER_SETTINGS_KEY,
    queryFn: () => apiGet<UserSettings>("/me/settings"),
  });
}

export function useUpdateUserSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: UserSettingsInput) =>
      apiSend<UserSettings>("/me/settings", "PUT", input),
    onSuccess: (settings) => {
      queryClient.setQueryData(USER_SETTINGS_KEY, settings);
    },
  });
}
