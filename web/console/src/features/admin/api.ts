import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut } from "@/api/client";
import type {
  AdminApiKeyView,
  ChannelCreateInput,
  ApiKeyCreateInput,
  ApiKeyPolicyInput,
  ApiKeyPolicyView,
  ApiKeyUpdateInput,
  AuditLogView,
  ChannelGroupInput,
  ChannelGroupView,
  ChannelInput,
  ChannelView,
  ConfigTemplateCreateInput,
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
  MutationResponse,
  ProxyInput,
  ProxyView,
  ReloadResponse,
  SystemSettings,
  SystemSettingsInput,
  UserInput,
} from "@/api/types";

type ListResult<T> = ReturnType<typeof useQuery<T[]>>;

function makeList<T>(basePath: string, key: readonly string[]) {
  return () =>
    useQuery({
      queryKey: key,
      queryFn: () => apiGet<T[]>(basePath),
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
export const useUpdateUser = makeUpdate<UserInput>("/users", USERS_KEY, userDetailKey);

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
export const useModels = makeList<ControlPlaneModel>("/models", MODELS_KEY);
export const useModel = makeDetail<ControlPlaneModel>("/models", modelDetailKey);
export const useCreateModel = makeCreate<ModelInput, MutationResponse>("/models", MODELS_KEY);
export const useUpdateModel = makeUpdate<ModelInput>("/models", MODELS_KEY, modelDetailKey);

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
export const useUpdateChannelGroup = makeUpdate<ChannelGroupInput>(
  "/routing/channel-groups",
  GROUPS_KEY,
  groupDetailKey,
);

// ---- Channels ----
const CHANNELS_KEY = ["console", "channels"] as const;
const channelDetailKey = (id: string) => ["console", "channels", id] as const;
export const useChannels = makeList<ChannelView>("/routing/channels", CHANNELS_KEY);
export const useChannel = makeDetail<ChannelView>("/routing/channels", channelDetailKey);
export const useCreateChannel = makeCreate<ChannelCreateInput, MutationResponse>(
  "/routing/channels",
  CHANNELS_KEY,
);
export const useUpdateChannel = makeUpdate<ChannelInput>(
  "/routing/channels",
  CHANNELS_KEY,
  channelDetailKey,
);

// ---- Model Rules ----
const RULES_KEY = ["console", "model-rules"] as const;
const ruleDetailKey = (id: string) => ["console", "model-rules", id] as const;
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
export const useCreateProxy = makeCreate<ProxyInput, MutationResponse>("/network/proxies", PROXIES_KEY);
export const useUpdateProxy = makeUpdate<ProxyInput>(
  "/network/proxies",
  PROXIES_KEY,
  proxyDetailKey,
);

// ---- Config Templates ----
const TEMPLATES_KEY = ["console", "config-templates"] as const;
const templateDetailKey = (id: string) => ["console", "config-templates", id] as const;
export const useConfigTemplates = makeList<ConfigTemplateView>(
  "/transforms/templates",
  TEMPLATES_KEY,
);
export const useConfigTemplate = makeDetail<ConfigTemplateView>(
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

// ---- Combined reference snapshot for forms ----
export function useControlPlaneLists() {
  return useQuery({
    queryKey: ["console", "control-plane-lists"] as const,
    queryFn: async (): Promise<ControlPlaneLists> => {
      const [users, models, api_keys, api_key_policies, channel_groups, channels, model_rules, proxies, config_templates] =
        await Promise.all([
          apiGet<ControlPlaneUser[]>("/users"),
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
