import { useEffect, useMemo, useState } from "react";
import {
  CheckCheck,
  Download,
  FileUp,
  KeyRound,
  Plus,
  Power,
  PowerOff,
  Trash2,
} from "lucide-react";
import { NavigationLink } from "@/components/shared/navigation-link";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";
import { toast } from "sonner";
import type {
  CodexCredentialImportInput,
  CodexCredentialView,
} from "@/api/types";
import { ApiError } from "@/api/errors";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { PageHeader } from "@/components/shared/page-header";
import { ResourceTable } from "@/components/shared/resource-table";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  useProxies,
} from "@/features/admin/api";
import {
  useBatchUpdateCodexCredentials,
  useCodexCredentials,
  useCompleteCodexOauth,
  useDeleteCodexCredential,
  useExportCodexCredentials,
  useImportCodexCredential,
  useRefreshCodexCredential,
  useRefreshCodexQuota,
  useResetCodexQuota,
  useStartCodexOauth,
} from "./api";

import { buildCodexCredentialColumns } from "./codex-oauth-columns";
import {
  ConnectOauthDialog,
  EditCredentialDialog,
  ImportTokensDialog,
  QuotaWindowHistoryDialog,
} from "./codex-oauth-dialogs";
import {
  downloadJson,
  EMPTY_IMPORT,
  EMPTY_SETTINGS,
  errorMessage,
  MAX_BATCH_SIZE,
  parseSettings,
  quotaResetOutcomeMessage,
  type CredentialSettingsState,
  type ImportState,
} from "./codex-oauth-helpers";
export default function CodexOauthPage() {
  const { t } = useI18n();
  const origin = usePageOrigin();
  const proxiesQuery = useProxies();
  const credentials = useCodexCredentials();
  const startOauth = useStartCodexOauth();
  const completeOauth = useCompleteCodexOauth();
  const importCredential = useImportCodexCredential();
  const exportCredentials = useExportCodexCredentials();
  const batchUpdate = useBatchUpdateCodexCredentials();
  const deleteCredential = useDeleteCodexCredential();
  const refreshCredential = useRefreshCodexCredential();
  const refreshQuota = useRefreshCodexQuota();
  const resetQuota = useResetCodexQuota();

  const [oauthOpen, setOauthOpen] = useState(false);
  const [oauthSettings, setOauthSettings] =
    useState<CredentialSettingsState>(EMPTY_SETTINGS);
  const [oauthFlow, setOauthFlow] = useState<{
    id: string;
    authorizationUrl: string;
    expiresAt: string;
  } | null>(null);
  const [callbackUrl, setCallbackUrl] = useState("");
  const [importOpen, setImportOpen] = useState(false);
  const [exportIds, setExportIds] = useState<string[] | null>(null);
  const [importState, setImportState] = useState<ImportState>(EMPTY_IMPORT);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [deleteTarget, setDeleteTarget] =
    useState<CodexCredentialView | null>(null);
  const [resetTarget, setResetTarget] =
    useState<CodexCredentialView | null>(null);
  const [historyTarget, setHistoryTarget] =
    useState<CodexCredentialView | null>(null);
  const [batchDeleteOpen, setBatchDeleteOpen] = useState(false);

  const proxies = useMemo(
    () =>
      (proxiesQuery.data ?? []).map((proxy) => ({
        id: proxy.id,
        name: proxy.name,
      })),
    [proxiesQuery.data],
  );
  const selectedCredentials = useMemo(
    () =>
      (credentials.data ?? []).filter((credential) =>
        selected.has(credential.id),
      ),
    [credentials.data, selected],
  );
  const allSelected =
    (credentials.data?.length ?? 0) > 0 &&
    selectedCredentials.length === credentials.data?.length;
  const exceedsBatchLimit = selectedCredentials.length > MAX_BATCH_SIZE;

  useEffect(() => {
    const available = new Set(
      (credentials.data ?? []).map((credential) => credential.id),
    );
    setSelected((current) => {
      const next = new Set([...current].filter((id) => available.has(id)));
      return next.size === current.size &&
        [...next].every((id) => current.has(id))
        ? current
        : next;
    });
  }, [credentials.data]);

  const toggleCredential = (credential: CodexCredentialView) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(credential.id)) next.delete(credential.id);
      else next.add(credential.id);
      return next;
    });
  };

  const closeOauth = () => {
    setOauthOpen(false);
    setOauthSettings(EMPTY_SETTINGS);
    setOauthFlow(null);
    setCallbackUrl("");
  };

  const beginOauth = async () => {
    const input = parseSettings(oauthSettings);
    if (!input) {
      toast.error(t("Enter a label and quota threshold from 1 to 100."));
      return;
    }
    try {
      const result = await startOauth.mutateAsync(input);
      setOauthFlow({
        id: result.flow_id,
        authorizationUrl: result.authorization_url,
        expiresAt: result.expires_at,
      });
    } catch (error) {
      toast.error(errorMessage(error));
    }
  };

  const finishOauth = async () => {
    if (!oauthFlow || !callbackUrl.trim()) {
      toast.error(t("Paste the complete callback URL."));
      return;
    }
    try {
      await completeOauth.mutateAsync({
        flowId: oauthFlow.id,
        input: { callback_url: callbackUrl.trim() },
      });
      toast.success(t("Codex credential connected."));
      closeOauth();
    } catch (error) {
      toast.error(errorMessage(error));
    }
  };

  const importTokens = async () => {
    const settings = parseSettings(importState);
    if (
      !settings ||
      !importState.access_token.trim() ||
      !importState.refresh_token.trim()
    ) {
      toast.error(t("Complete all required credential fields."));
      return;
    }
    const input: CodexCredentialImportInput = {
      ...settings,
      enabled: true,
      access_token: importState.access_token.trim(),
      refresh_token: importState.refresh_token.trim(),
      account_id: importState.account_id.trim() || null,
      user_id: importState.user_id.trim() || null,
    };
    if (importState.id_token.trim()) {
      input.id_token = importState.id_token.trim();
    }
    try {
      await importCredential.mutateAsync(input);
      toast.success(t("Codex credential imported."));
      setImportOpen(false);
      setImportState(EMPTY_IMPORT);
    } catch (error) {
      toast.error(errorMessage(error));
    }
  };

  const exportSelectedCredentials = async () => {
    if (exportIds === null) return;
    try {
      const bundle = await exportCredentials.mutateAsync({
        credential_ids: exportIds,
        include_proxies: true,
      });
      downloadJson(
        bundle,
        `codex-credentials-${new Date().toISOString().slice(0, 10)}.json`,
      );
      toast.success(t("Codex credentials exported."));
      setExportIds(null);
    } catch (error) {
      toast.error(errorMessage(error));
    }
  };

  const runBatch = async (operation: "enable" | "disable" | "delete") => {
    if (selectedCredentials.length === 0) return;
    if (exceedsBatchLimit) {
      toast.error(
        t("Batch actions support up to {count} selected credentials.", {
          count: MAX_BATCH_SIZE,
        }),
      );
      return;
    }
    try {
      const result = await batchUpdate.mutateAsync({
        items: selectedCredentials.map((credential) => ({
          id: credential.id,
          updated_at: credential.updated_at,
        })),
        operation,
      });
      const message =
        operation === "enable"
          ? "Enabled {count} credentials."
          : operation === "disable"
            ? "Disabled {count} credentials."
            : "Deleted {count} credentials.";
      toast.success(t(message, { count: result.updated_ids.length }));
      setSelected(new Set());
      setBatchDeleteOpen(false);
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        await credentials.refetch();
      }
      toast.error(errorMessage(error));
    }
  };

  const deleteSingleCredential = async () => {
    const target = deleteTarget;
    if (!target) return;
    try {
      await deleteCredential.mutateAsync({
        id: target.id,
        ifMatch: `"${target.updated_at}"`,
      });
      toast.success(t("Deleted {label}.", { label: target.label }));
      setDeleteTarget(null);
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        await credentials.refetch();
      }
      toast.error(errorMessage(error));
    }
  };

  const runRefresh = async (
    action: "token" | "quota",
    credential: CodexCredentialView,
  ) => {
    try {
      if (action === "token") {
        await refreshCredential.mutateAsync(credential.id);
        toast.success(t("Token refreshed."));
      } else {
        await refreshQuota.mutateAsync(credential.id);
        toast.success(t("Quota refreshed."));
      }
    } catch (error) {
      toast.error(errorMessage(error));
    }
  };

  const runQuotaReset = async () => {
    const target = resetTarget;
    if (!target) return;
    try {
      const result = await resetQuota.mutateAsync(target.id);
      const message = quotaResetOutcomeMessage(result.outcome);
      if (!result.quota_refreshed) {
        toast.warning(
          t(
            "{message} The follow-up quota refresh failed; automatic polling will retry.",
            { message },
          ),
        );
      } else if (result.outcome === "reset") {
        toast.success(
          t("{message} {count} windows reset.", {
            message,
            count: result.windows_reset,
          }),
        );
      } else {
        toast.info(message);
      }
      setResetTarget(null);
    } catch (error) {
      toast.error(errorMessage(error));
    }
  };

  const managementEnabled = !credentials.error;

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageHeader
        embedded
        title={t("Codex")}
        actions={
          <>
            <Button
              variant="outline"
              disabled={!managementEnabled || !credentials.data?.length || exportCredentials.isPending}
              onClick={() => setExportIds([])}
            >
              <Download data-icon="inline-start" />
              {t("Export credentials")}
            </Button>
            <NavigationLink to={withReturnTo("/admin/routing/upstream-credentials/codex/import", origin)}>
              <FileUp data-icon="inline-start" />
              {t("Advanced import")}
            </NavigationLink>
            <Button
              variant="outline"
              disabled={!managementEnabled}
              onClick={() => setImportOpen(true)}
            >
              <KeyRound data-icon="inline-start" /> {t("Import tokens")}
            </Button>
            <Button
              disabled={!managementEnabled}
              onClick={() => setOauthOpen(true)}
            >
              <Plus data-icon="inline-start" /> {t("Connect account")}
            </Button>
          </>
        }
      />

      <div className="grid gap-4 sm:grid-cols-3">
        <Card>
          <CardHeader>
            <CardDescription>{t("Credentials")}</CardDescription>
            <CardTitle className="text-2xl">
              {credentials.data?.length ?? "—"}
            </CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription>{t("Ready for new requests")}</CardDescription>
            <CardTitle className="text-2xl">
              {credentials.data?.filter(
                (credential) => credential.runtime_status === "active",
              ).length ?? "—"}
            </CardTitle>
          </CardHeader>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription>{t("Draining or unavailable")}</CardDescription>
            <CardTitle className="text-2xl">
              {credentials.data?.filter(
                (credential) => credential.runtime_status !== "active",
              ).length ?? "—"}
            </CardTitle>
          </CardHeader>
        </Card>
      </div>

      <Card className="min-w-0">
        <CardHeader>
          <CardTitle>{t("Managed credentials")}</CardTitle>
          <CardDescription>
            {t(
              "Bind imported credentials in Channel configuration. Existing sticky sessions may keep using a draining credential.",
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={credentials.isLoading}
            error={credentials.error}
            isEmpty={(credentials.data?.length ?? 0) === 0}
            emptyTitle={t("No Codex credentials")}
            emptyDescription={t("Connect an account with OAuth or import an existing token set.")}
          >
            <div className="mb-4 flex flex-wrap items-center gap-2">
              <Button
                variant="outline"
                onClick={() => {
                  if (allSelected) setSelected(new Set());
                  else {
                    setSelected(
                      new Set(
                        (credentials.data ?? []).map(
                          (credential) => credential.id,
                        ),
                      ),
                    );
                  }
                }}
              >
                <CheckCheck data-icon="inline-start" />
                {allSelected ? t("Clear selection") : t("Select all")}
              </Button>
              <Button
                variant="outline"
                disabled={
                  selectedCredentials.length === 0 ||
                  exportCredentials.isPending
                }
                onClick={() =>
                  setExportIds(
                    selectedCredentials.map((credential) => credential.id),
                  )
                }
              >
                <Download data-icon="inline-start" />
                {t("Export selected ({count})", {
                  count: selectedCredentials.length,
                })}
              </Button>
              <Button
                variant="outline"
                disabled={
                  selectedCredentials.length === 0 ||
                  exceedsBatchLimit ||
                  batchUpdate.isPending
                }
                onClick={() => void runBatch("enable")}
              >
                <Power data-icon="inline-start" />
                {t("Enable")}
              </Button>
              <Button
                variant="outline"
                disabled={
                  selectedCredentials.length === 0 ||
                  exceedsBatchLimit ||
                  batchUpdate.isPending
                }
                onClick={() => void runBatch("disable")}
              >
                <PowerOff data-icon="inline-start" />
                {t("Disable")}
              </Button>
              <Button
                variant="destructive"
                disabled={
                  selectedCredentials.length === 0 ||
                  exceedsBatchLimit ||
                  batchUpdate.isPending
                }
                onClick={() => setBatchDeleteOpen(true)}
              >
                <Trash2 data-icon="inline-start" />
                {t("Delete selected")}
              </Button>
              <span
                className={
                  exceedsBatchLimit
                    ? "text-sm text-destructive"
                    : "text-sm text-muted-foreground"
                }
              >
                {t("{count} selected", { count: selectedCredentials.length })}
                {exceedsBatchLimit
                  ? ` · ${t(
                      "Batch actions support up to {count} selected credentials.",
                      { count: MAX_BATCH_SIZE },
                    )}`
                  : ""}
              </span>
            </div>
            <ResourceTable
              columns={buildCodexCredentialColumns({
                t,
                selected,
                allSelected,
                proxies,
                resetQuotaPending: resetQuota.isPending,
                refreshQuotaPending: refreshQuota.isPending,
                refreshCredentialPending: refreshCredential.isPending,
                deleteCredentialPending: deleteCredential.isPending,
                onToggleAll: () => {
                  if (allSelected) setSelected(new Set());
                  else {
                    setSelected(
                      new Set((credentials.data ?? []).map((credential) => credential.id)),
                    );
                  }
                },
                onToggleCredential: toggleCredential,
                onOpenHistory: setHistoryTarget,
                onEdit: setEditingId,
                onResetQuota: setResetTarget,
                onRefresh: (kind, credential) => void runRefresh(kind, credential),
                onDelete: setDeleteTarget,
              })}
              rows={credentials.data ?? []}
              rowKey={(credential) => credential.id}
              pagination={false}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      <ConnectOauthDialog
        open={oauthOpen}
        onOpenChange={(open) => (open ? setOauthOpen(true) : closeOauth())}
        oauthFlow={oauthFlow}
        callbackUrl={callbackUrl}
        onCallbackUrlChange={setCallbackUrl}
        oauthSettings={oauthSettings}
        onOauthSettingsChange={(patch) =>
          setOauthSettings((current) => ({ ...current, ...patch }))
        }
        proxies={proxies}
        isPending={startOauth.isPending || completeOauth.isPending}
        onBegin={() => void beginOauth()}
        onFinish={() => void finishOauth()}
      />

      <ImportTokensDialog
        open={importOpen}
        onOpenChange={setImportOpen}
        importState={importState}
        onImportStateChange={(patch) =>
          setImportState((current) => ({ ...current, ...patch }))
        }
        proxies={proxies}
        isPending={importCredential.isPending}
        onImport={() => void importTokens()}
      />
      <EditCredentialDialog
        credentialId={editingId}
        proxies={proxies}
        onClose={() => setEditingId(null)}
      />

      <QuotaWindowHistoryDialog
        credential={historyTarget}
        onClose={() => setHistoryTarget(null)}
      />

      <AlertDialog
        open={exportIds !== null}
        onOpenChange={(open) => {
          if (!open) setExportIds(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("Export Codex credentials?")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                "The downloaded JSON contains raw ID, access, refresh, and proxy credentials. Store it as a secret and delete it when no longer needed.",
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("Cancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => void exportSelectedCredentials()}
              disabled={exportCredentials.isPending}
            >
              {exportIds?.length
                ? t("Export selected")
                : t("Export all")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <ConfirmDialog
        open={resetTarget !== null}
        onOpenChange={(open) => {
          if (!open) setResetTarget(null);
        }}
        title={t("Consume an OpenAI reset credit?")}
        description={
          resetTarget
            ? t(
                "This calls OpenAI's reset-credit endpoint for {label}. One available credit may be consumed and both quota windows may restart. Available credits: {count}.",
                {
                  label: resetTarget.label,
                  count:
                    resetTarget.quota_reset_credits_available ?? "unknown",
                },
              )
            : ""
        }
        confirmLabel={t("Consume reset credit")}
        confirmDisabled={resetQuota.isPending}
        onConfirm={() => void runQuotaReset()}
      />

      <ConfirmDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
        title={t("Delete Codex credential?")}
        description={
          deleteTarget
            ? t(
                "{label} will stop receiving requests and its stored OAuth tokens will be cleared. This cannot be undone.",
                { label: deleteTarget.label },
              )
            : ""
        }
        confirmLabel={t("Delete credential")}
        destructive
        confirmDisabled={deleteCredential.isPending}
        onConfirm={() => void deleteSingleCredential()}
      />

      <ConfirmDialog
        open={batchDeleteOpen}
        onOpenChange={setBatchDeleteOpen}
        title={t("Delete selected Codex credentials?")}
        description={t(
          "{count} credentials will stop receiving requests and their stored OAuth tokens will be cleared. This cannot be undone.",
          { count: selectedCredentials.length },
        )}
        confirmLabel={t("Delete selected")}
        destructive
        confirmDisabled={batchUpdate.isPending}
        onConfirm={() => void runBatch("delete")}
      />
    </div>
  );
}
