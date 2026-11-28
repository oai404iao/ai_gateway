import type { ProxyView } from "@/api/types";
import { ApiError } from "@/api/errors";
import { useI18n } from "@/app/i18n";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { CodexCredentialImportDraft, ImportedProxyDraft } from "./codex-import";

export const NO_PROXY = "__none__";
export const MAX_FILE_BYTES = 5 * 1024 * 1024;
export const MAX_FILES = 20;

export function CredentialEditorDialog({
  credential,
  onChange,
  onClose,
}: {
  credential: CodexCredentialImportDraft | null;
  onChange: (patch: Partial<CodexCredentialImportDraft>) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  return (
    <Dialog
      open={Boolean(credential)}
      onOpenChange={(open) => !open && onClose()}
    >
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("Edit credential secrets")}</DialogTitle>
          <DialogDescription>
            {t(
              "Review or replace imported tokens and identity fields. Values remain in memory until import.",
            )}
          </DialogDescription>
        </DialogHeader>
        {credential ? (
          <FieldGroup>
            <Field orientation="horizontal">
              <FieldLabel htmlFor="codex-import-enabled">
                {t("Enabled after import")}
              </FieldLabel>
              <Switch
                id="codex-import-enabled"
                checked={credential.enabled}
                onCheckedChange={(enabled) =>
                  onChange({ enabled: Boolean(enabled) })
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-account-id">
                {t("Account ID override (optional)")}
              </FieldLabel>
              <Input
                id="codex-import-account-id"
                value={credential.account_id}
                onChange={(event) =>
                  onChange({ account_id: event.target.value })
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-user-id">
                {t("User ID override (optional)")}
              </FieldLabel>
              <Input
                id="codex-import-user-id"
                value={credential.user_id}
                onChange={(event) => onChange({ user_id: event.target.value })}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-id-token">
                {t("ID token (optional)")}
              </FieldLabel>
              <Textarea
                id="codex-import-id-token"
                value={credential.id_token}
                onChange={(event) =>
                  onChange({ id_token: event.target.value })
                }
                rows={4}
                spellCheck={false}
                className="font-mono text-xs"
              />
              <FieldDescription>
                {t(
                  "When omitted, the gateway reads identity claims from the access token.",
                )}
              </FieldDescription>
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-access-token">
                {t("Access token")}
              </FieldLabel>
              <Textarea
                id="codex-import-access-token"
                value={credential.access_token}
                onChange={(event) =>
                  onChange({ access_token: event.target.value })
                }
                rows={5}
                spellCheck={false}
                className="font-mono text-xs"
                aria-invalid={!credential.access_token.trim()}
              />
              {!credential.access_token.trim() ? (
                <FieldError>{t("Access token is required.")}</FieldError>
              ) : null}
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-refresh-token">
                {t("Refresh token")}
              </FieldLabel>
              <Textarea
                id="codex-import-refresh-token"
                value={credential.refresh_token}
                onChange={(event) =>
                  onChange({ refresh_token: event.target.value })
                }
                rows={5}
                spellCheck={false}
                className="font-mono text-xs"
                aria-invalid={!credential.refresh_token.trim()}
              />
              {!credential.refresh_token.trim() ? (
                <FieldError>{t("Refresh token is required.")}</FieldError>
              ) : null}
            </Field>
          </FieldGroup>
        ) : null}
        <DialogFooter>
          <Button onClick={onClose}>{t("Done")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function SummaryCard({ label, value }: { label: string; value: number }) {
  return (
    <Card>
      <CardHeader>
        <CardDescription>{label}</CardDescription>
        <CardTitle className="text-2xl">{value}</CardTitle>
      </CardHeader>
    </Card>
  );
}

export function ImportStatusBadge({
  credential,
}: {
  credential: CodexCredentialImportDraft;
}) {
  const { t } = useI18n();
  if (credential.status === "imported") {
    return <Badge variant="success">{t("Imported")}</Badge>;
  }
  if (credential.status === "failed") {
    return <Badge variant="destructive">{t("Failed")}</Badge>;
  }
  if (credential.status === "importing") {
    return <Badge variant="info">{t("Importing")}</Badge>;
  }
  return <Badge variant="secondary">{t("Pending")}</Badge>;
}

export function credentialErrors(
  credential: CodexCredentialImportDraft,
  validProxyIds: Set<string>,
): string[] {
  const errors = credential.errors.filter((error) =>
    error.startsWith("Duplicate of import row"),
  );
  const threshold = Number(credential.quota_threshold_percent);
  if (!credential.label.trim()) errors.push("Label is required.");
  if (!credential.access_token.trim()) errors.push("Access token is required.");
  if (!credential.refresh_token.trim()) errors.push("Refresh token is required.");
  if (!Number.isInteger(threshold) || threshold < 1 || threshold > 100) {
    errors.push("Quota threshold must be from 1 to 100.");
  }
  if (credential.proxy_id && !validProxyIds.has(credential.proxy_id)) {
    errors.push("The assigned proxy is missing or disabled.");
  }
  return [...new Set(errors)];
}

export function credentialWarnings(
  credential: CodexCredentialImportDraft,
  importedProxies: ImportedProxyDraft[],
): string[] {
  const warnings = [...credential.warnings];
  if (
    credential.source_proxy_key &&
    !credential.proxy_id &&
    importedProxies.some(
      (proxy) => proxy.source_key === credential.source_proxy_key,
    )
  ) {
    warnings.push(
      "The source proxy is not mapped; this credential will use a direct connection.",
    );
  }
  return [...new Set(warnings)];
}

export function matchExistingProxy(
  imported: ImportedProxyDraft,
  existing: ProxyView[],
): ProxyView | undefined {
  return (
    existing.find(
      (proxy) =>
        proxy.name === imported.name && proxy.proxy_url === imported.proxy_url,
    ) ??
    existing.find((proxy) => proxy.proxy_url === imported.proxy_url)
  );
}

export function codexImportError(
  error: unknown,
  t: (key: string, values?: Record<string, string | number>) => string,
): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "codex_account_changed":
        return t("The supplied workspace or user ID does not match the token.");
      case "codex_network_policy_invalid":
        return t("The selected outbound proxy is unavailable.");
      case "codex_refresh_token_invalid":
        return t("The refresh token is no longer valid.");
      case "codex_models_invalid":
        return t("The account returned no supported Codex models.");
      case "codex_credential_invalid":
        return t("The credential tokens are invalid.");
    }
    return error.message;
  }
  return error instanceof Error ? error.message : t("Import failed");
}
