import type {
  CodexCredentialView,
  CodexQuotaResetOutcome,
  CodexQuotaWindowPeriod,
} from "@/api/types";
import { ApiError } from "@/api/errors";
import { translate } from "@/app/i18n";

export const NO_PROXY = "__none__";
export const MAX_BATCH_SIZE = 100;

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.isConflict) {
      return translate(
        "This Codex credential is already connected or changed elsewhere.",
      );
    }
    switch (error.code) {
      case "codex_oauth_state_mismatch":
        return translate("The OAuth callback state did not match. Start a new authorization.");
      case "codex_oauth_flow_expired":
        return translate("The OAuth flow expired. Start a new authorization.");
      case "codex_refresh_token_invalid":
        return translate("The refresh token is no longer valid. Connect the account again.");
      case "codex_account_changed":
        return translate(
          "The refreshed token belongs to a different Codex workspace or member.",
        );
      case "codex_network_policy_invalid":
        return translate("The selected outbound proxy is unavailable.");
    }
  }
  return error instanceof Error ? error.message : "Request failed";
}

export interface CredentialSettingsState {
  label: string;
  proxy_id: string;
  quota_threshold_percent: string;
}

export interface ImportState extends CredentialSettingsState {
  id_token: string;
  access_token: string;
  refresh_token: string;
  account_id: string;
  user_id: string;
}

export interface EditState extends CredentialSettingsState {
  enabled: boolean;
}

export const EMPTY_SETTINGS: CredentialSettingsState = {
  label: "",
  proxy_id: "",
  quota_threshold_percent: "95",
};

export const EMPTY_IMPORT: ImportState = {
  ...EMPTY_SETTINGS,
  id_token: "",
  access_token: "",
  refresh_token: "",
  account_id: "",
  user_id: "",
};

export function parsePositiveInteger(value: string): number | null {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

export function parseSettings(
  state: CredentialSettingsState,
): { label: string; proxy_id: string | null; quota_threshold_percent: number } | null {
  const threshold = parsePositiveInteger(state.quota_threshold_percent);
  if (
    !state.label.trim() ||
    threshold === null ||
    threshold > 100
  ) {
    return null;
  }
  return {
    label: state.label.trim(),
    proxy_id: state.proxy_id || null,
    quota_threshold_percent: threshold,
  };
}

export function credentialStatusVariant(
  status: CodexCredentialView["runtime_status"],
): "success" | "warning" | "destructive" | "default" {
  if (status === "active") return "success";
  if (status === "draining") return "warning";
  if (status === "unavailable") return "destructive";
  return "default";
}

export function quotaSummary(credential: CodexCredentialView): string {
  if (credential.quota_limit_reached) return translate("Limit reached");
  if (credential.quota_allowed === false) return translate("Not allowed");
  if (credential.primary_used_percent !== null) {
    return translate("{percent}% used", {
      percent: credential.primary_used_percent,
    });
  }
  return translate("Not checked");
}

export function credentialStatusLabel(
  status: CodexCredentialView["runtime_status"],
): string {
  if (status === "active") return translate("Active");
  if (status === "draining") return translate("Draining");
  if (status === "unavailable") return translate("Unavailable");
  return translate("Disabled");
}

export function quotaResetReasonLabel(
  reason: CodexQuotaWindowPeriod["reset_reason"],
): string {
  if (reason === "natural") return translate("Natural reset");
  if (reason === "manual") return translate("Manual reset credit");
  if (reason === "openai_official") return translate("OpenAI official reset");
  return translate("Current period");
}

export function quotaResetReasonVariant(
  reason: CodexQuotaWindowPeriod["reset_reason"],
): "success" | "warning" | "info" | "secondary" {
  if (reason === "manual") return "warning";
  if (reason === "openai_official") return "info";
  if (reason === "natural") return "secondary";
  return "success";
}

export function quotaResetOutcomeMessage(outcome: CodexQuotaResetOutcome): string {
  if (outcome === "reset") return translate("OpenAI reset credit consumed.");
  if (outcome === "nothing_to_reset") {
    return translate("No active quota window needed a reset.");
  }
  if (outcome === "no_credit") {
    return translate("No OpenAI reset credit is available.");
  }
  return translate("This reset request was already redeemed.");
}


export function downloadJson(value: unknown, filename: string) {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(value, null, 2)], {
      type: "application/json",
    }),
  );
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}
