import type {
  ApiFormat,
  SelectionStrategy,
  UpstreamAuthKind,
  UserRole,
} from "@/api/types";

export const API_FORMATS: readonly ApiFormat[] = [
  "open_ai_chat_completions",
  "open_ai_responses",
];

export const SELECTION_STRATEGIES: readonly SelectionStrategy[] = [
  "weighted_random",
  "weighted_round_robin",
];

export const UPSTREAM_AUTH_KINDS: readonly UpstreamAuthKind[] = ["bearer", "header"];

/** Permissions recognized by the data plane. */
export const PERMISSIONS = ["proxy", "models.read"] as const;

export const ROLES: readonly UserRole[] = ["user", "admin"];

export const USER_STATUSES = ["active", "invited", "disabled"] as const;

export const API_KEY_STATUSES = ["active", "disabled", "revoked"] as const;

export function apiFormatLabel(value: ApiFormat): string {
  return value === "open_ai_chat_completions" ? "Chat Completions" : "Responses";
}

export function roleLabel(value: UserRole): string {
  return value === "admin" ? "Administrator" : "User";
}

export function selectionStrategyLabel(value: SelectionStrategy): string {
  return value === "weighted_random" ? "Weighted random" : "Weighted round-robin";
}

export function upstreamAuthKindLabel(value: UpstreamAuthKind): string {
  return value === "bearer" ? "Bearer token" : "Custom header";
}

export function outcomeLabel(value: string): string {
  switch (value) {
    case "success":
      return "Success";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
    default:
      return value;
  }
}

export function outcomeVariant(
  value: string,
): "default" | "secondary" | "destructive" {
  if (value === "success") return "secondary";
  if (value === "failed") return "destructive";
  return "default";
}
