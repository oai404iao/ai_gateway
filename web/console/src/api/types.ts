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
export type SelectionStrategy = S["SelectionStrategy"];
export type UpstreamAuthKind = S["UpstreamAuthKind"];
export type ModelSyncAction = S["ModelSyncAction"];
export type ChannelStatusWindow = S["ChannelStatusWindow"];
export type StatisticsGranularity = S["StatisticsGranularity"];

// Auth + shared responses
export type ErrorBody = S["ErrorBody"];
export type ConsoleUser = S["ConsoleUser"];
export type LoginResponse = S["LoginResponse"];
export type ConsoleProfile = S["ConsoleProfile"];
export type ConsoleSession = S["ConsoleSession"];
export type InvitationResponse = S["InvitationResponse"];
export type MutationResponse = S["MutationResponse"];
export type ReloadResponse = S["ReloadResponse"];

// Resources (views)
export type ApiKeyView = S["ApiKeyView"];
export type AdminApiKeyView = S["AdminApiKeyView"];
export type ApiKeyPolicyView = S["ApiKeyPolicyView"];
export type ControlPlaneUser = S["ControlPlaneUser"];
export type ControlPlaneModel = S["ControlPlaneModel"];
export type ChannelGroupView = S["ChannelGroupView"];
export type ChannelView = S["ChannelView"];
export type ModelRuleView = S["ModelRuleView"];
export type ProxyView = S["ProxyView"];
export type ConfigTemplateView = S["ConfigTemplateView"];

// Observability + catalog
export type RequestLogView = S["RequestLogView"];
export type AuditLogView = S["AuditLogView"];
export type ChannelStatusReport = S["ChannelStatusReport"];
export type ChannelStatusModelMetric = S["ChannelStatusModelMetric"];
export type ChannelStatusChannel = S["ChannelStatusChannel"];
export type ChannelStatusChannelModel = S["ChannelStatusChannelModel"];
export type ChannelStatusBucket = S["ChannelStatusBucket"];
export type CurrencyAmount = S["CurrencyAmount"];
export type CostStatisticsReport = S["CostStatisticsReport"];
export type CostStatisticsSummary = S["CostStatisticsSummary"];
export type CostStatisticsBucket = S["CostStatisticsBucket"];
export type CostStatisticsBucketModel = S["CostStatisticsBucketModel"];
export type CostStatisticsModel = S["CostStatisticsModel"];
export type ModelSyncPreview = S["ModelSyncPreview"];
export type ModelSyncPreviewModel = S["ModelSyncPreviewModel"];
export type ModelImportResponse = S["ModelImportResponse"];

// Request bodies
export type LoginInput = S["LoginInput"];
export type ActivateInvitationInput = S["ActivateInvitationInput"];
export type ProfileUpdateInput = S["ProfileUpdateInput"];
export type PasswordChangeInput = S["PasswordChangeInput"];
export type RevokeInput = S["RevokeInput"];
export type SelfApiKeyCreateInput = S["SelfApiKeyCreateInput"];
export type SelfApiKeyUpdateInput = S["SelfApiKeyUpdateInput"];
export type InviteUserInput = S["InviteUserInput"];
export type UserInput = S["UserInput"];
export type ApiKeyPolicyInput = S["ApiKeyPolicyInput"];
export type ApiKeyCreateInput = S["ApiKeyCreateInput"];
export type ApiKeyUpdateInput = S["ApiKeyUpdateInput"];
export type ModelInput = S["ModelInput"];
export type ChannelGroupInput = S["ChannelGroupInput"];
export type ChannelCreateInput = S["ChannelCreateInput"];
export type ChannelInput = S["ChannelInput"];
export type ModelRuleInput = S["ModelRuleInput"];
export type ProxyCreateInput = S["ProxyCreateInput"];
export type ProxyInput = S["ProxyInput"];
export type ConfigTemplateCreateInput = S["ConfigTemplateCreateInput"];
export type ConfigTemplateInput = S["ConfigTemplateInput"];
export type ModelSyncSelection = S["ModelSyncSelection"];
export type ModelImportRequest = S["ModelImportRequest"];
export type ModelSyncPreviewRequest = S["ModelSyncPreviewRequest"];
export type ListQuery = S["ListQuery"];

// Client-side aggregate (not a server response). Assembled by
// `useControlPlaneLists` from the individual list endpoints above.
export interface ControlPlaneLists {
  users: ControlPlaneUser[];
  models: ControlPlaneModel[];
  api_keys: AdminApiKeyView[];
  api_key_policies: ApiKeyPolicyView[];
  channel_groups: ChannelGroupView[];
  channels: ChannelView[];
  model_rules: ModelRuleView[];
  proxies: ProxyView[];
  config_templates: ConfigTemplateView[];
}
