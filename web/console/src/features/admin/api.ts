import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiGet, apiGetDetail, apiPost, apiPut, apiSend } from "@/api/client";
import type {
  AdminApiKeyView,
  ApiKeyCreateInput,
  ApiKeyPolicyInput,
  ApiKeyPolicyView,
  ApiKeyUpdateInput,
  AuditLogView,
  ChannelModelDiscoveryInput,
  ChannelModelDiscoveryResponse,
  ConfigTemplateCreateInput,
  ConfigTemplateDetailView,
  ConfigTemplateInput,
  ConfigTemplateView,
  ControlPlaneModel,
  ControlPlaneUser,
  InviteUserInput,
  InvitationResponse,
  ModelImportRequest,
  ModelImportResponse,
  ModelInput,
  ModelRuleCreateInput,
  ModelSyncPreview,
  ModelSyncPreviewRequest,
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
  UpstreamCredentialView,
  UpstreamAccessView,
  UpstreamAccessInput,
  RoutingGroupView,
  RoutingGroupInput,
  LogicalChannelView,
  LogicalChannelInput,
  ChannelCapabilityView,
  ChannelCapabilityInput,
  OperationRuleView,
  RoutingProfileView,
  OperationRuleInput,
  UpstreamCredentialDetail,
  UpstreamCredentialInput,
  UpstreamCredentialCreateInput,
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

const ACCESSES_KEY = ["console", "upstream-accesses"] as const;
const accessDetailKey = (id: string) => [...ACCESSES_KEY, id] as const;
const ACCESSES_PATH = "/routing/accesses";
export const useUpstreamAccesses = makeList<UpstreamAccessView>(ACCESSES_PATH, ACCESSES_KEY);
export const useUpstreamAccess = makeDetail<UpstreamAccessView>(ACCESSES_PATH, accessDetailKey);
export const useCreateUpstreamAccess = makeCreate<UpstreamAccessInput, MutationResponse>(ACCESSES_PATH, ACCESSES_KEY);
export const useUpdateUpstreamAccess = makeUpdate<UpstreamAccessInput>(ACCESSES_PATH, ACCESSES_KEY, accessDetailKey);

// ---- Canonical routing topology ----
const ROUTING_GROUPS_KEY = ["console", "routing-groups"] as const;
const routingGroupDetailKey = (id: string) => [...ROUTING_GROUPS_KEY, id] as const;
const ROUTING_GROUPS_PATH = "/routing/groups";
export const useRoutingGroups = makeList<RoutingGroupView>(ROUTING_GROUPS_PATH, ROUTING_GROUPS_KEY);
export const useRoutingGroup = makeDetail<RoutingGroupView>(ROUTING_GROUPS_PATH, routingGroupDetailKey);
export const useCreateRoutingGroup = makeCreate<RoutingGroupInput, MutationResponse>(
  ROUTING_GROUPS_PATH,
  ROUTING_GROUPS_KEY,
);
export function useUpdateRoutingGroup(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: RoutingGroupInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`${ROUTING_GROUPS_PATH}/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ROUTING_GROUPS_KEY });
      void queryClient.invalidateQueries({ queryKey: routingGroupDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: LOGICAL_CHANNELS_KEY });
    },
  });
}
export function useDeleteRoutingGroup(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`${ROUTING_GROUPS_PATH}/${id}`, "DELETE", undefined, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ROUTING_GROUPS_KEY });
      void queryClient.invalidateQueries({ queryKey: LOGICAL_CHANNELS_KEY });
      queryClient.removeQueries({ queryKey: routingGroupDetailKey(id) });
    },
  });
}

const LOGICAL_CHANNELS_KEY = ["console", "logical-channels"] as const;
const logicalChannelDetailKey = (id: string) => [...LOGICAL_CHANNELS_KEY, id] as const;
const LOGICAL_CHANNELS_PATH = "/routing/logical-channels";
export const useLogicalChannels = makeList<LogicalChannelView>(
  LOGICAL_CHANNELS_PATH,
  LOGICAL_CHANNELS_KEY,
);
export const useLogicalChannel = makeDetail<LogicalChannelView>(
  LOGICAL_CHANNELS_PATH,
  logicalChannelDetailKey,
);
export const useCreateLogicalChannel = makeCreate<LogicalChannelInput, MutationResponse>(
  LOGICAL_CHANNELS_PATH,
  LOGICAL_CHANNELS_KEY,
);
export function useUpdateLogicalChannel(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: LogicalChannelInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`${LOGICAL_CHANNELS_PATH}/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: LOGICAL_CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: logicalChannelDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
    },
  });
}
export function useDeleteLogicalChannel(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`${LOGICAL_CHANNELS_PATH}/${id}`, "DELETE", undefined, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: LOGICAL_CHANNELS_KEY });
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
      queryClient.removeQueries({ queryKey: logicalChannelDetailKey(id) });
    },
  });
}

const CAPABILITIES_KEY = ["console", "channel-capabilities"] as const;
const capabilityDetailKey = (id: string) => [...CAPABILITIES_KEY, id] as const;
const CAPABILITIES_PATH = "/routing/capabilities";
export function useBatchUpdateCapabilities() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: import("@/api/types").CapabilityBatchUpdateInput) =>
      apiPost<import("@/api/types").CapabilityBatchUpdateResponse>(`${CAPABILITIES_PATH}/batch`, input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
      void queryClient.invalidateQueries({ queryKey: OPERATION_RULES_KEY });
    },
  });
}
export function useRecoverCapability(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`${CAPABILITIES_PATH}/${id}/recover`, "POST", undefined, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
    },
  });
}
export const useChannelCapabilities = makeList<ChannelCapabilityView>(
  CAPABILITIES_PATH,
  CAPABILITIES_KEY,
);
export const useChannelCapability = makeDetail<ChannelCapabilityView>(
  CAPABILITIES_PATH,
  capabilityDetailKey,
);
export const useCreateChannelCapability = makeCreate<ChannelCapabilityInput, MutationResponse>(
  CAPABILITIES_PATH,
  CAPABILITIES_KEY,
);
export function useUpdateChannelCapability(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: ChannelCapabilityInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`${CAPABILITIES_PATH}/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
      void queryClient.invalidateQueries({ queryKey: capabilityDetailKey(id) });
      void queryClient.invalidateQueries({ queryKey: OPERATION_RULES_KEY });
    },
  });
}
export function useDeleteChannelCapability(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`${CAPABILITIES_PATH}/${id}`, "DELETE", undefined, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
      void queryClient.invalidateQueries({ queryKey: OPERATION_RULES_KEY });
      queryClient.removeQueries({ queryKey: capabilityDetailKey(id) });
    },
  });
}

const OPERATION_RULES_KEY = ["console", "operation-rules"] as const;
const ROUTING_PROFILES_KEY = ["console", "routing-profiles"] as const;
export const useRoutingProfiles = makeList<RoutingProfileView>("/routing/profiles", ROUTING_PROFILES_KEY);
export const useCreateRoutingProfile = makeCreate<ModelRuleCreateInput, MutationResponse>(
  "/routing/profiles", ROUTING_PROFILES_KEY,
);
const operationRuleDetailKey = (id: string) => [...OPERATION_RULES_KEY, id] as const;
const OPERATION_RULES_PATH = "/routing/operation-rules";
export const useOperationRules = makeList<OperationRuleView>(
  OPERATION_RULES_PATH,
  OPERATION_RULES_KEY,
);
export const useOperationRule = makeDetail<OperationRuleView>(
  OPERATION_RULES_PATH,
  operationRuleDetailKey,
);
export const useCreateOperationRule = makeCreate<OperationRuleInput, MutationResponse>(
  OPERATION_RULES_PATH,
  OPERATION_RULES_KEY,
);
export function useUpdateOperationRule(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ input, ifMatch }: { input: OperationRuleInput; ifMatch: string }) =>
      apiPut<MutationResponse>(`${OPERATION_RULES_PATH}/${id}`, input, ifMatch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: OPERATION_RULES_KEY });
      void queryClient.invalidateQueries({ queryKey: operationRuleDetailKey(id) });
    },
  });
}

const CREDENTIALS_KEY = ["console", "upstream-credentials"] as const;
const credentialDetailKey = (id: string) => [...CREDENTIALS_KEY, id] as const;
const CREDENTIALS_PATH = "/routing/upstream-credentials";
export const useUpstreamCredentials = makeList<UpstreamCredentialView>(CREDENTIALS_PATH, CREDENTIALS_KEY);
export const useUpstreamCredential = makeDetail<UpstreamCredentialDetail>(CREDENTIALS_PATH, credentialDetailKey);
export const useCreateUpstreamCredential = makeCreate<UpstreamCredentialCreateInput, MutationResponse>(CREDENTIALS_PATH, CREDENTIALS_KEY);
export const useUpdateUpstreamCredential = makeUpdate<UpstreamCredentialInput>(CREDENTIALS_PATH, CREDENTIALS_KEY, credentialDetailKey);
export function useDeleteUpstreamCredential(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (ifMatch: string) => apiSend<MutationResponse>(`${CREDENTIALS_PATH}/${id}`, "DELETE", undefined, { ifMatch }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: CREDENTIALS_KEY });
      queryClient.removeQueries({ queryKey: credentialDetailKey(id) });
    },
  });
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
      void queryClient.invalidateQueries({ queryKey: ADMIN_KEYS_KEY });
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
      void queryClient.invalidateQueries({ queryKey: USERS_KEY });
      void queryClient.invalidateQueries({ queryKey: REGISTRATION_CODES_KEY });
      void queryClient.invalidateQueries({
        queryKey: ["console", "me", "api-key-options"],
      });
      void queryClient.invalidateQueries({
        queryKey: ["console", "me", "codex-quotas"],
      });
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
      void queryClient.invalidateQueries({ queryKey: ROUTING_PROFILES_KEY });
      void queryClient.invalidateQueries({ queryKey: OPERATION_RULES_KEY });
    },
  });
}
export function useDeleteModel(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/models/${id}`, "DELETE", undefined, {
        ifMatch,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: MODELS_KEY });
      void queryClient.invalidateQueries({ queryKey: ROUTING_PROFILES_KEY });
      void queryClient.invalidateQueries({ queryKey: OPERATION_RULES_KEY });
      void queryClient.invalidateQueries({ queryKey: CAPABILITIES_KEY });
      void queryClient.invalidateQueries({
        queryKey: ["console", "control-plane-lists"],
      });
      queryClient.removeQueries({ queryKey: modelDetailKey(id) });
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
export function useDeleteAdminApiKey(id: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ifMatch }: { ifMatch: string }) =>
      apiSend<MutationResponse>(`/api-keys/${id}`, "DELETE", undefined, {
        ifMatch,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ADMIN_KEYS_KEY });
      queryClient.removeQueries({ queryKey: adminKeyDetailKey(id) });
    },
  });
}

export function useDiscoverChannelModels() {
  return useMutation({
    mutationFn: (input: ChannelModelDiscoveryInput) =>
      apiPost<ChannelModelDiscoveryResponse>("/routing/channels/models/discover", input),
  });
}
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
