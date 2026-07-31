// Console API TypeScript contracts.
//
// This module is a thin re-export shim over `src/api/generated/console-v1.d.ts`,
// which is produced by `pnpm generate:api` from `docs/openapi/console-v1.yaml`.
// The generated file is the single source of truth for request/response
// shapes; do not hand-edit it. Extend the OpenAPI spec and regenerate instead.
//
// The only hand-maintained type here is `ControlPlaneLists`, a client-side
// aggregate assembled by `useControlPlaneLists`; it is not a server response.
//
// Naming: OpenAPI `components.schemas.*` names intentionally match the export
// names below so existing `import type { ConsoleUser } from "@/api/types"`
// callers keep working unchanged.

import type { components } from "@/api/generated/console-v1";

type S = components["schemas"];

// Enums / scalar aliases
export type UserRole = S["UserRole"];
export type ApiFormat = S["ApiFormat"];
export type ApiOperation = S["ApiOperation"];
export type SelectionStrategy = S["SelectionStrategy"];
export type ConnectorKind = S["ConnectorKind"];
export type UpstreamAuthKind = S["UpstreamAuthKind"];
export type ScheduledTestingMode = S["ScheduledTestingMode"];
export type RequestLogSource = S["RequestLogSource"];
export type RequestProtocol = S["RequestProtocol"];
export type ModelSyncAction = S["ModelSyncAction"];
export type ChannelStatusWindow = S["ChannelStatusWindow"];
export type StatisticsGranularity = S["StatisticsGranularity"];
export type SpendLeaderboardPeriod = S["SpendLeaderboardPeriod"];

// Auth + shared responses
export type ErrorBody = S["ErrorBody"];
export type ConsoleUser = S["ConsoleUser"];
export type LoginResponse = S["LoginResponse"];
export type ConsoleProfile = S["ConsoleProfile"];
export type UserSettings = S["UserSettings"];
export type ConsoleSession = S["ConsoleSession"];
export type ConsoleSessionState = S["ConsoleSessionState"];
export type InvitationResponse = S["InvitationResponse"];
export type RegistrationInvitationCodeCreateResponse =
  S["RegistrationInvitationCodeCreateResponse"];
export type MutationResponse = S["MutationResponse"];
export type UserBatchUpdateResponse = S["UserBatchUpdateResponse"];
export type ReloadResponse = S["ReloadResponse"];
export type SystemSettings = S["SystemSettings"];
export type SystemLoadReport = S["SystemLoadReport"];
export type SystemHostLoad = S["SystemHostLoad"];
export type SystemProcessLoad = S["SystemProcessLoad"];
export type SystemRuntimeLoad = S["SystemRuntimeLoad"];
export type SystemQueuesLoad = S["SystemQueuesLoad"];
export type SystemQueueLoad = S["SystemQueueLoad"];
export type SystemRequestLogLoad = S["SystemRequestLogLoad"];
export type SystemWebSocketLoad = S["SystemWebSocketLoad"];
export type SystemDatabaseLoad = S["SystemDatabaseLoad"];
export type SystemDatabasePoolLoad = S["SystemDatabasePoolLoad"];
export type SystemRequestRetrySettings = S["SystemRequestRetrySettings"];
export type SystemSessionAffinityKeySource = S["SystemSessionAffinityKeySource"];
export type SystemSessionAffinityRule = S["SystemSessionAffinityRule"];
export type SystemSessionAffinitySettings = S["SystemSessionAffinitySettings"];
export type SystemWebSocketSettings = S["SystemWebSocketSettings"];
export type SessionAffinityCacheReport = S["SessionAffinityCacheReport"];
export type SessionAffinityCacheClearResponse = S["SessionAffinityCacheClearResponse"];

// Resources (views)
export type ApiKeyView = S["ApiKeyView"];
export type AdminApiKeyView = S["AdminApiKeyView"];
export type ApiKeyPolicyView = S["ApiKeyPolicyView"];
export type SelfApiKeyOptions = S["SelfApiKeyOptions"];
export type SelfApiKeyGroupOption = S["SelfApiKeyGroupOption"];
export type SelfApiKeyChannelOption = S["SelfApiKeyChannelOption"];
export type ApiHostsView = S["ApiHostsView"];
export type ControlPlaneUser = S["ControlPlaneUser"];
export type UserGroupView = S["UserGroupView"];
export type RegistrationInvitationCodeView = S["RegistrationInvitationCodeView"];
export type ControlPlaneModel = S["ControlPlaneModel"];
export type AdvancedBilling = S["AdvancedBilling"];
export type LongContextTier = S["LongContextTier"];
export type RequestBillingMultiplier = S["RequestBillingMultiplier"];
export type ChannelGroupView = S["ChannelGroupView"];
export type ChannelView = S["ChannelView"];
export type ChannelDetailView = S["ChannelDetailView"];
export type ChannelModelDiscoveryResponse = S["ChannelModelDiscoveryResponse"];
export type ChannelBatchUpdateResponse = S["ChannelBatchUpdateResponse"];
export type CodexCredentialStatus = S["CodexCredentialStatus"];
export type CodexCredentialView = S["CodexCredentialView"];
export type CodexCredentialBatchResponse = S["CodexCredentialBatchResponse"];
export type CodexOauthStartResponse = S["CodexOauthStartResponse"];
export type CodexCredentialExportBundle = S["CodexCredentialExportBundle"];
export type CodexCredentialExportProxy = S["CodexCredentialExportProxy"];
export type CodexCredentialExportItem = S["CodexCredentialExportItem"];
export type ModelRuleView = S["ModelRuleView"];
export type ProxyView = S["ProxyView"];
export type ProxyTestResponse = S["ProxyTestResponse"];
export type ConfigTemplateView = S["ConfigTemplateView"];
export type ConfigTemplateDetailView = S["ConfigTemplateDetailView"];

// Observability + catalog
export type RequestLogView = S["RequestLogView"];
export type AuditLogView = S["AuditLogView"];
export type PersonalUsageReport = S["PersonalUsageReport"];
export type PersonalUsageDay = S["PersonalUsageDay"];
export type ChannelStatusReport = S["ChannelStatusReport"];
export type ChannelStatusModelMetric = S["ChannelStatusModelMetric"];
export type ChannelStatusChannel = S["ChannelStatusChannel"];
export type ChannelStatusChannelModel = S["ChannelStatusChannelModel"];
export type ChannelStatusBucket = S["ChannelStatusBucket"];
export type CostStatisticsReport = S["CostStatisticsReport"];
export type CostStatisticsSummary = S["CostStatisticsSummary"];
export type CostStatisticsBucket = S["CostStatisticsBucket"];
export type CostStatisticsBucketModel = S["CostStatisticsBucketModel"];
export type CostStatisticsModel = S["CostStatisticsModel"];
export type CostStatisticsChannel = S["CostStatisticsChannel"];
export type SpendLeaderboardReport = S["SpendLeaderboardReport"];
export type SpendLeaderboardEntry = S["SpendLeaderboardEntry"];
export type ModelSyncPreview = S["ModelSyncPreview"];
export type ModelSyncPreviewModel = S["ModelSyncPreviewModel"];
export type ModelImportResponse = S["ModelImportResponse"];

// Request bodies
export type LoginInput = S["LoginInput"];
export type RegisterInput = S["RegisterInput"];
export type ActivateInvitationInput = S["ActivateInvitationInput"];
export type ProfileUpdateInput = S["ProfileUpdateInput"];
export type UserSettingsInput = S["UserSettingsInput"];
export type PasswordChangeInput = S["PasswordChangeInput"];
export type RevokeInput = S["RevokeInput"];
export type SelfApiKeyCreateInput = S["SelfApiKeyCreateInput"];
export type SelfApiKeyUpdateInput = S["SelfApiKeyUpdateInput"];
export type InviteUserInput = S["InviteUserInput"];
export type RegistrationInvitationCodeCreateInput =
  S["RegistrationInvitationCodeCreateInput"];
export type RegistrationInvitationCodeUpdateInput =
  S["RegistrationInvitationCodeUpdateInput"];
export type UserInput = S["UserInput"];
export type UserUpdateInput = S["UserUpdateInput"];
export type UserBatchUpdateTarget = S["UserBatchUpdateTarget"];
export type UserBalanceBatchChange = S["UserBalanceBatchChange"];
export type UserBatchChanges = S["UserBatchChanges"];
export type UserBatchUpdateInput = S["UserBatchUpdateInput"];
export type UserGroupInput = S["UserGroupInput"];
export type ApiKeyPolicyInput = S["ApiKeyPolicyInput"];
export type ApiKeyCreateInput = S["ApiKeyCreateInput"];
export type ApiKeyUpdateInput = S["ApiKeyUpdateInput"];
export type ModelInput = S["ModelInput"];
export type ChannelGroupInput = S["ChannelGroupInput"];
export type ChannelCreateInput = S["ChannelCreateInput"];
export type ChannelInput = S["ChannelInput"];
export type ChannelModelDiscoveryInput = S["ChannelModelDiscoveryInput"];
export type ChannelBatchUpdateTarget = S["ChannelBatchUpdateTarget"];
export type ChannelBatchChanges = S["ChannelBatchChanges"];
export type ChannelBatchUpdateInput = S["ChannelBatchUpdateInput"];
export type ChannelRecoverInput = S["ChannelRecoverInput"];
export type CodexOauthStartInput = S["CodexOauthStartInput"];
export type CodexOauthCompleteInput = S["CodexOauthCompleteInput"];
export type CodexCredentialImportInput = S["CodexCredentialImportInput"];
export type CodexCredentialExportInput = S["CodexCredentialExportInput"];
export type CodexCredentialUpdateInput = S["CodexCredentialUpdateInput"];
export type CodexCredentialBatchOperation =
  S["CodexCredentialBatchOperation"];
export type CodexCredentialBatchTarget = S["CodexCredentialBatchTarget"];
export type CodexCredentialBatchInput = S["CodexCredentialBatchInput"];
export type ModelRuleInput = S["ModelRuleInput"];
export type ProxyCreateInput = S["ProxyCreateInput"];
export type ProxyInput = S["ProxyInput"];
export type ProxyTestInput = S["ProxyTestInput"];
export type ConfigTemplateCreateInput = S["ConfigTemplateCreateInput"];
export type ConfigTemplateInput = S["ConfigTemplateInput"];
export type ModelSyncSelection = S["ModelSyncSelection"];
export type ModelImportRequest = S["ModelImportRequest"];
export type ModelSyncPreviewRequest = S["ModelSyncPreviewRequest"];
export type ListQuery = S["ListQuery"];
export type SystemSettingsInput = S["SystemSettingsInput"];

// Client-side aggregate (not a server response). Assembled by
// `useControlPlaneLists` from the individual list endpoints above.
export interface ControlPlaneLists {
  users: ControlPlaneUser[];
  user_groups: UserGroupView[];
  models: ControlPlaneModel[];
  api_keys: AdminApiKeyView[];
  api_key_policies: ApiKeyPolicyView[];
  channel_groups: ChannelGroupView[];
  channels: ChannelView[];
  model_rules: ModelRuleView[];
  proxies: ProxyView[];
  config_templates: ConfigTemplateView[];
}
