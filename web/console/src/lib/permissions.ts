import type {
  ApiFormat,
  ConnectorKind,
  RequestCompression,
  SelectionStrategy,
  UpstreamAuthKind,
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
  "openai_compatible",
  "codex_oauth",
];

export const REQUEST_COMPRESSIONS: readonly RequestCompression[] = ["default", "zstd"];

export const UPSTREAM_AUTH_KINDS: readonly UpstreamAuthKind[] = ["none", "bearer", "header"];

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
  return value === "openai_compatible"
    ? translate("OpenAI-compatible")
    : translate("Codex OAuth");
}

export function requestCompressionLabel(value: RequestCompression): string {
  return value === "default" ? translate("Default") : "Zstandard (zstd)";
}

export function upstreamAuthKindLabel(value: UpstreamAuthKind): string {
  if (value === "none") return translate("No upstream auth");
  return value === "bearer" ? translate("Bearer token") : translate("Custom header");
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
