import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut, apiSend } from "@/api/client";
import type {
  AdminApiKeyView,
  ChannelBatchUpdateInput,
  ChannelBatchUpdateResponse,
  ChannelRecoverInput,
  ChannelCreateInput,
  ApiKeyCreateInput,
  ApiKeyPolicyInput,
  ApiKeyPolicyView,
  ApiKeyUpdateInput,
  AuditLogView,
  ChannelGroupInput,
  ChannelGroupView,
  ChannelDetailView,
  ChannelInput,
  ChannelModelDiscoveryInput,
  ChannelModelDiscoveryResponse,
  ChannelView,
  ConfigTemplateCreateInput,
  ConfigTemplateDetailView,
  ConfigTemplateInput,
  ConfigTemplateView,
  ControlPlaneLists,
  ControlPlaneModel,
  ControlPlaneUser,
  InviteUserInput,
  InvitationResponse,
  ModelImportRequest,
  ModelImportResponse,
  ModelInput,
  ModelRuleInput,
  ModelRuleView,
  ModelSyncPreview,
  ModelSyncPreviewRequest,
  McpServerCreateInput,
  McpServerInput,
  McpServerView,
  MutationResponse,
  ProxyCreateInput,
  ProxyInput,
  ProxyTestInput,
  ProxyTestResponse,
  ProxyView,
  RegistrationInvitationCodeCreateInput,
  RegistrationInvitationCodeCreateResponse,
  RegistrationInvitationCodeUpdateInput,
  RegistrationInvitationCodeView,
  ReloadResponse,
  SessionAffinityCacheClearResponse,
  SessionAffinityCacheReport,
  SystemSettings,
  SystemSettingsInput,
  TemporaryPasswordInput,
  TemporaryPasswordResponse,
  UserBatchUpdateInput,
  UserBatchUpdateResponse,
  UserGroupInput,
  UserGroupView,
  UserUpdateInput,
} from "@/api/types";

type ListResult<T> = ReturnType<typeof useQuery<T[]>>;

function makeList<T>(basePath: string, key: readonly string[]) {
  return (enabled = true) =>
    useQuery({
      queryKey: key,
      queryFn: () => apiGet<T[]>(basePath),
      enabled,
    }) as ListResult<T>;
}

function makeDetail<T>(basePath: string, key: (id: string) => readonly string[]) {
  return (id: string) => {
    const query = useQuery({
      queryKey: key(id),
      queryFn: () => apiGetDetail<T>(`${basePath}/${id}`),
      enabled: Boolean(id) && id !== "new",
    });
    return {
      data: query.data,
      etag: query.data?.etag ?? "",
      isLoading: query.isLoading,
      error: query.error,
      refetch: query.refetch,
    };
  };
}

function makeCreate<TBody, TResp extends MutationResponse>(
  basePath: string,
  listKey: readonly string[],
) {
  return () => {
    const queryClient = useQueryClient();
    return useMutation({
      mutationFn: (input: TBody) => apiPost<TResp>(basePath, input),
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: listKey });
      },
    });
  };
}

function makeUpdate<TBody>(
  basePath: string,
  listKey: readonly string[],
  detailKey: (id: string) => readonly string[],
) {
  return (id: string) => {
    const queryClient = useQueryClient();
    return useMutation({
      mutationFn: ({ input, ifMatch }: { input: TBody; ifMatch: string }) =>
        apiPut<MutationResponse>(`${basePath}/${id}`, input, ifMatch),
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: listKey });
        void queryClient.invalidateQueries({ queryKey: detailKey(id) });
      },
    });
  };
}

// ---- Users ----
const USERS_KEY = ["console", "users"] as const;
const userDetailKey = (id: string) => ["console", "users", id] as const;
export const useUsers = makeList<ControlPlaneUser>("/users", USERS_KEY);
export const useUser = makeDetail<ControlPlaneUser>("/users", userDetailKey);
export function useInviteUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: InviteUserInput) => apiPost<InvitationResponse>("/users", input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
    },
  });
}
export function useReissueUserInvitation(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => apiPost<InvitationResponse>(`/users/${id}/invitation`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
      void queryClient.invalidateQueries({ queryKey: userDetailKey(id) });
    },
  });
}
export function useIssueTemporaryPassword(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: TemporaryPasswordInput) =>
      apiPost<TemporaryPasswordResponse>(`/users/${id}/temporary-password`, input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
      void queryClient.invalidateQueries({ queryKey: userDetailKey(id) });
    },
  });
}
export function useUpdateUser(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: UserUpdateInput; ifMatch: string }) =>
      apiSend<MutationResponse>(`/users/${id}`, "PATCH", input, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
      void queryClient.invalidateQueries({ queryKey: userDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: ["console", "me", "settings"] });
    },
  });
}
export function useBatchUpdateUsers() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: UserBatchUpdateInput) =>
      apiPost<UserBatchUpdateResponse>("/users/batch", input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
    },
  });
}
export function useDeleteUser(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/users/${id}`, "DELETE", undefined, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
      void queryClient.removeQueries({ queryKey: userDetailKey(id) });
    },
  });
}

// ---- User Groups ----
const USER_GROUPS_KEY = ["console", "user-groups"] as const;
const userGroupDetailKey = (id: string) => ["console", "user-groups", id] as const;
export const useUserGroups = makeList<UserGroupView>("/user-groups", USER_GROUPS_KEY);
export const useUserGroup = makeDetail<UserGroupView>("/user-groups", userGroupDetailKey);
export const useCreateUserGroup = makeCreate<UserGroupInput, MutationResponse>(
  "/user-groups",
  USER_GROUPS_KEY,
);
export function useUpdateUserGroup(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: UserGroupInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`/user-groups/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USER_GROUPS_KEY });
      void queryClient.invalidateQueries({ queryKey: userGroupDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
      void queryClient.invalidateQueries({
        queryKey: ["console", "me", "codex-quotas"],
      });
    },
  });
}
export function useDeleteUserGroup(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/user-groups/${id}`, "DELETE", undefined, {
        ifMatch,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: USER_GROUPS_KEY });
      void queryClient.removeQueries({ queryKey: userGroupDetailKey(id) });
    },
  });
}

// ---- Registration Invitation Codes ----
const REGISTRATION_CODES_KEY = [
  "console",
  "registration-invitation-codes",
] as const;
const registrationCodeDetailKey = (id: string) =>
  ["console", "registration-invitation-codes", id] as const;
export const useRegistrationInvitationCodes =
  makeList<RegistrationInvitationCodeView>(
    "/registration-invitation-codes",
    REGISTRATION_CODES_KEY,
  );
export const useRegistrationInvitationCode =
  makeDetail<RegistrationInvitationCodeView>(
    "/registration-invitation-codes",
    registrationCodeDetailKey,
  );
export const useCreateRegistrationInvitationCode = makeCreate<
  RegistrationInvitationCodeCreateInput,
  RegistrationInvitationCodeCreateResponse
>("/registration-invitation-codes", REGISTRATION_CODES_KEY);
export const useUpdateRegistrationInvitationCode =
  makeUpdate<RegistrationInvitationCodeUpdateInput>(
    "/registration-invitation-codes",
    REGISTRATION_CODES_KEY,
    registrationCodeDetailKey,
  );

// ---- API Key Policies ----
const POLICIES_KEY = ["console", "api-key-policies"] as const;
const policyDetailKey = (id: string) => ["console", "api-key-policies", id] as const;
export const useApiKeyPolicies = makeList<ApiKeyPolicyView>("/api-key-policies", POLICIES_KEY);
export const useApiKeyPolicy = makeDetail<ApiKeyPolicyView>("/api-key-policies", policyDetailKey);
export const useCreateApiKeyPolicy = makeCreate<ApiKeyPolicyInput, MutationResponse>(
  "/api-key-policies",
  POLICIES_KEY,
);
export const useUpdateApiKeyPolicy = makeUpdate<ApiKeyPolicyInput>(
  "/api-key-policies",
  POLICIES_KEY,
  policyDetailKey,
);

// ---- Models ----
const MODELS_KEY = ["console", "models"] as const;
const modelDetailKey = (id: string) => ["console", "models", id] as const;
const RULES_KEY = ["console", "model-rules"] as const;
const ruleDetailKey = (id: string) => ["console", "model-rules", id] as const;
export const useModels = makeList<ControlPlaneModel>("/models", MODELS_KEY);
export const useModel = makeDetail<ControlPlaneModel>("/models", modelDetailKey);
export const useCreateModel = makeCreate<ModelInput, MutationResponse>("/models", MODELS_KEY);
export function useUpdateModel(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: ModelInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`/models/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: MODELS_KEY });
      void queryClient.invalidateQueries({ queryKey: modelDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
    },
  });
}

// ---- Admin API Keys ----
const ADMIN_KEYS_KEY = ["console", "admin-api-keys"] as const;
const adminKeyDetailKey = (id: string) => ["console", "admin-api-keys", id] as const;
export const useAdminApiKeys = makeList<AdminApiKeyView>("/api-keys", ADMIN_KEYS_KEY);
export const useAdminApiKey = makeDetail<AdminApiKeyView>("/api-keys", adminKeyDetailKey);
export const useCreateAdminApiKey = makeCreate<ApiKeyCreateInput, MutationResponse>(
  "/api-keys",
  ADMIN_KEYS_KEY,
);
export const useUpdateAdminApiKey = makeUpdate<ApiKeyUpdateInput>(
  "/api-keys",
  ADMIN_KEYS_KEY,
  adminKeyDetailKey,
);
export function useRevokeAdminApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, reason }: { id: string; reason: { reason: string } }) =>
      apiPost<MutationResponse>(`/api-keys/${id}/revoke`, reason),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ADMIN_KEYS_KEY });
    },
  });
}

// ---- Channel Groups ----
const GROUPS_KEY = ["console", "channel-groups"] as const;
const groupDetailKey = (id: string) => ["console", "channel-groups", id] as const;
export const useChannelGroups = makeList<ChannelGroupView>(
  "/routing/channel-groups",
  GROUPS_KEY,
);
export const useChannelGroup = makeDetail<ChannelGroupView>("/routing/channel-groups", groupDetailKey);
export const useCreateChannelGroup = makeCreate<ChannelGroupInput, MutationResponse>(
  "/routing/channel-groups",
  GROUPS_KEY,
);
export function useUpdateChannelGroup(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: ChannelGroupInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`/routing/channel-groups/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: GROUPS_KEY });
      void queryClient.invalidateQueries({ queryKey: groupDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
    },
  });
}
export function useSetChannelGroupEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      group,
      enabled,
    }: {
      group: ChannelGroupView;
      enabled: boolean;
    }) =>
      apiPut<MutationResponse>(
        `/routing/channel-groups/${group.id}`,
        {
          name: group.name,
          api_format: group.api_format,
          connector_kind: group.connector_kind,
          request_compression: group.request_compression,
          priority: group.priority,
          selection_strategy: group.selection_strategy,
          enabled,
        } satisfies ChannelGroupInput,
        `"${group.updated_at}"`,
      ),
    onSettled: (_data, _error, variables) => {
      void queryClient.invalidateQueries({ queryKey: GROUPS_KEY });
      void queryClient.invalidateQueries({
        queryKey: groupDetailKey(variables.group.id),
      });
      void queryClient.invalidateQueries({ queryKey: CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
      void queryClient.invalidateQueries({
        queryKey: ["console", "me", "api-key-options"],
      });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}

// ---- Channels ----
const CHANNELS_KEY = ["console", "channels"] as const;
const channelDetailKey = (id: string) => ["console", "channels", id] as const;
export const useChannels = makeList<ChannelView>("/routing/channels", CHANNELS_KEY);
export const useChannel = makeDetail<ChannelDetailView>("/routing/channels", channelDetailKey);
export function useCreateChannel() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: ChannelCreateInput) =>
      apiPost<MutationResponse>("/routing/channels", input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
    },
  });
}
export function useUpdateChannel(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: ChannelInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`/routing/channels/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: channelDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
    },
  });
}
export function useDiscoverChannelModels() {
  return useMutation({
    mutationFn: (input: ChannelModelDiscoveryInput) =>
      apiPost<ChannelModelDiscoveryResponse>("/routing/channels/models/discover", input),
  });
}
export function useBatchUpdateChannels() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: ChannelBatchUpdateInput) =>
      apiPost<ChannelBatchUpdateResponse>("/routing/channels/batch", input),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
    },
  });
}
export function useRecoverChannel() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: ChannelRecoverInput }) =>
      apiPost<MutationResponse>(`/routing/channels/${id}/recover`, input),
    onSettled: (_data, _error, variables) => {
      void queryClient.invalidateQueries({ queryKey: CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: channelDetailKey(variables.id) });
      void queryClient.invalidateQueries({ queryKey: RULES_KEY });
    },
  });
}

// ---- Model Rules ----
export const useModelRules = makeList<ModelRuleView>("/routing/model-rules", RULES_KEY);
export const useModelRule = makeDetail<ModelRuleView>("/routing/model-rules", ruleDetailKey);
export const useCreateModelRule = makeCreate<ModelRuleInput, MutationResponse>(
  "/routing/model-rules",
  RULES_KEY,
);
export const useUpdateModelRule = makeUpdate<ModelRuleInput>(
  "/routing/model-rules",
  RULES_KEY,
  ruleDetailKey,
);

// ---- Proxies ----
const PROXIES_KEY = ["console", "proxies"] as const;
const proxyDetailKey = (id: string) => ["console", "proxies", id] as const;
export const useProxies = makeList<ProxyView>("/network/proxies", PROXIES_KEY);
export const useProxy = makeDetail<ProxyView>("/network/proxies", proxyDetailKey);
export function useCreateProxy() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: ProxyCreateInput) =>
      apiPost<MutationResponse>("/network/proxies", input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: PROXIES_KEY });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}
export function useUpdateProxy(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: ProxyInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`/network/proxies/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: PROXIES_KEY });
      void queryClient.invalidateQueries({ queryKey: proxyDetailKey(id) });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}
export function useDeleteProxy(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/network/proxies/${id}`, "DELETE", undefined, {
        ifMatch,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: PROXIES_KEY });
      void queryClient.removeQueries({ queryKey: proxyDetailKey(id) });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
    },
  });
}
export function useTestProxy() {
  return useMutation({
    mutationFn: (input: ProxyTestInput) =>
      apiPost<ProxyTestResponse>("/network/proxies/test", input),
  });
}

// ---- Config Templates ----
const TEMPLATES_KEY = ["console", "config-templates"] as const;
const templateDetailKey = (id: string) => ["console", "config-templates", id] as const;
export const useConfigTemplates = makeList<ConfigTemplateView>(
  "/transforms/templates",
  TEMPLATES_KEY,
);
export const useConfigTemplate = makeDetail<ConfigTemplateDetailView>(
  "/transforms/templates",
  templateDetailKey,
);
export const useCreateConfigTemplate = makeCreate<ConfigTemplateCreateInput, MutationResponse>(
  "/transforms/templates",
  TEMPLATES_KEY,
);
export const useUpdateConfigTemplate = makeUpdate<ConfigTemplateInput>(
  "/transforms/templates",
  TEMPLATES_KEY,
  templateDetailKey,
);

// ---- MCP Servers ----
const MCP_SERVERS_KEY = ["console", "mcp-servers"] as const;
const mcpServerDetailKey = (id: string) =>
  ["console", "mcp-servers", id] as const;
export const useMcpServers = makeList<McpServerView>(
  "/mcp-servers",
  MCP_SERVERS_KEY,
);
export const useMcpServer = makeDetail<McpServerView>(
  "/mcp-servers",
  mcpServerDetailKey,
);
export const useCreateMcpServer = makeCreate<
  McpServerCreateInput,
  MutationResponse
>("/mcp-servers", MCP_SERVERS_KEY);
export const useUpdateMcpServer = makeUpdate<McpServerInput>(
  "/mcp-servers",
  MCP_SERVERS_KEY,
  mcpServerDetailKey,
);
export function useDeleteMcpServer(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/mcp-servers/${id}`, "DELETE", undefined, {
        ifMatch,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: MCP_SERVERS_KEY });
      void queryClient.removeQueries({ queryKey: mcpServerDetailKey(id) });
    },
  });
}

// ---- Catalog sync ----
export function useModelSyncPreview() {
  return useMutation({
    mutationFn: (input: ModelSyncPreviewRequest) =>
      apiPost<ModelSyncPreview>("/catalog/models/sync/preview", input),
  });
}
export function useApplyCatalogModels() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: ModelImportRequest) =>
      apiPost<ModelImportResponse>("/catalog/models/import", input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: MODELS_KEY });
    },
  });
}

// ---- Audit logs + system ----
export function useAuditLogs(limit: number) {
  return useQuery({
    queryKey: ["console", "audit-logs", limit] as const,
    queryFn: () => apiGet<AuditLogView[]>(`/audit-logs?limit=${limit}`),
  });
}
export function useReload() {
  return useMutation({
    mutationFn: () => apiPost<ReloadResponse>("/system/reload"),
  });
}
const SYSTEM_SETTINGS_KEY = ["console", "system-settings"] as const;
const SESSION_AFFINITY_CACHE_KEY = [
  "console",
  "system",
  "session-affinity-cache",
] as const;
export function useSystemSettings() {
  return useQuery({
    queryKey: SYSTEM_SETTINGS_KEY,
    queryFn: () => apiGetDetail<SystemSettings>("/system/settings"),
  });
}
export function useUpdateSystemSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: SystemSettingsInput; ifMatch: string }) =>
      apiPut<MutationResponse>("/system/settings", input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SYSTEM_SETTINGS_KEY });
    },
  });
}
export function useSessionAffinityCache() {
  return useQuery({
    queryKey: SESSION_AFFINITY_CACHE_KEY,
    queryFn: () =>
      apiGet<SessionAffinityCacheReport>("/system/session-affinity/cache"),
    refetchInterval: 5_000,
  });
}
export function useClearSessionAffinityCache() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (ruleName?: string) => {
      const query = ruleName
        ? `?${new URLSearchParams({ rule_name: ruleName }).toString()}`
        : "";
      return apiSend<SessionAffinityCacheClearResponse>(
        `/system/session-affinity/cache${query}`,
        "DELETE",
      );
    },
    onSuccess: (response) => {
      queryClient.setQueryData(SESSION_AFFINITY_CACHE_KEY, response.cache);
    },
  });
}

// ---- Combined reference snapshot for forms ----
export function useControlPlaneLists() {
  return useQuery({
    queryKey: ["console", "control-plane-lists"] as const,
    queryFn: async (): Promise<ControlPlaneLists> => {
      const [users, user_groups, models, api_keys, api_key_policies, channel_groups, channels, model_rules, proxies, config_templates] =
        await Promise.all([
          apiGet<ControlPlaneUser[]>("/users"),
          apiGet<UserGroupView[]>("/user-groups"),
          apiGet<ControlPlaneModel[]>("/models"),
          apiGet<AdminApiKeyView[]>("/api-keys"),
          apiGet<ApiKeyPolicyView[]>("/api-key-policies"),
          apiGet<ChannelGroupView[]>("/routing/channel-groups"),
          apiGet<ChannelView[]>("/routing/channels"),
          apiGet<ModelRuleView[]>("/routing/model-rules"),
          apiGet<ProxyView[]>("/network/proxies"),
          apiGet<ConfigTemplateView[]>("/transforms/templates"),
        ]);
      return {
        users,
        user_groups,
        models,
        api_keys,
        api_key_policies,
        channel_groups,
        channels,
        model_rules,
        proxies,
        config_templates,
      };
    },
  });
}
