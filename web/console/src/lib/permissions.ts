import type {
  ApiFormat,
  ApiOperation,
  ConnectorKind,
  RequestCompression,
  SelectionStrategy,
  UserRole,
} from "@/api/types";
import { translate } from "@/app/i18n";

export const API_FORMATS: readonly ApiFormat[] = [
  "open_ai_chat_completions",
  "open_ai_responses",
  "open_ai_images",
];

export const SELECTION_STRATEGIES: readonly SelectionStrategy[] = [
  "weighted_random",
  "weighted_round_robin",
];

export const CONNECTOR_KINDS: readonly ConnectorKind[] = [
  "general",
  "codex",
];

export const REQUEST_COMPRESSIONS: readonly RequestCompression[] = ["default", "zstd"];

export const API_OPERATIONS: readonly ApiOperation[] = [
  "chat_completion",
  "responses",
  "responses-ws",
  "web_search",
  "images_generation",
  "images_edit",
];


/** Permissions recognized by the data plane. */
export const PERMISSIONS = ["proxy", "models.read"] as const;

export const ROLES: readonly UserRole[] = ["user", "admin"];

export const USER_STATUSES = ["active", "invited", "suspended", "disabled"] as const;

export const API_KEY_STATUSES = ["active", "disabled", "revoked"] as const;

/** OpenAI API-format product terms intentionally remain in English. */
export function apiFormatLabel(value: ApiFormat): string {
  switch (value) {
    case "open_ai_chat_completions":
      return "Chat Completions";
    case "open_ai_responses":
      return "Responses";
    case "open_ai_images":
      return "Images";
  }
}

export function operationApiFormat(value: ApiOperation): ApiFormat {
  switch (value) {
    case "chat_completion":
      return "open_ai_chat_completions";
    case "responses":
    case "responses-ws":
    case "web_search":
      return "open_ai_responses";
    case "images_generation":
    case "images_edit":
      return "open_ai_images";
  }
}

export function roleLabel(value: UserRole): string {
  return value === "admin" ? translate("Administrator") : translate("User");
}

export function userStatusLabel(value: (typeof USER_STATUSES)[number]): string {
  switch (value) {
    case "active":
      return translate("Active");
    case "invited":
      return translate("Invited");
    case "suspended":
      return translate("Suspended");
    case "disabled":
      return translate("Disabled");
  }
}

export function selectionStrategyLabel(value: SelectionStrategy): string {
  return value === "weighted_random"
    ? translate("Weighted random")
    : translate("Weighted round-robin");
}

export function connectorKindLabel(value: ConnectorKind): string {
  return value === "general"
    ? translate("General")
    : "Codex";
}

export function requestCompressionLabel(value: RequestCompression): string {
  return value === "default" ? translate("Default") : "Zstandard (zstd)";
}

/** OpenAI operation names intentionally remain in English product terms. */
export function apiOperationLabel(value: ApiOperation): string {
  switch (value) {
    case "chat_completion":
      return "Chat Completions";
    case "responses":
      return "Responses";
    case "responses-ws":
      return "Responses WebSocket";
    case "web_search":
      return "Standalone web search";
    case "images_generation":
      return "Images generation";
    case "images_edit":
      return "Images edit";
  }
}

export function outcomeLabel(value: string): string {
  switch (value) {
    case "succeeded":
      return translate("Succeeded");
    case "failed":
      return translate("Failed");
    case "rejected":
      return translate("Rejected");
    case "cancelled":
      return translate("Cancelled");
    default:
      return value;
  }
}

export function outcomeVariant(
  value: string,
): "default" | "success" | "warning" | "destructive" {
  if (value === "succeeded") return "success";
  if (value === "failed") return "destructive";
  if (value === "rejected") return "warning";
  return "default";
}
