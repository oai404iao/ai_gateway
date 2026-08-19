import { useEffect, useMemo, useState } from "react";
import {
  ArrowLeft,
  CheckCheck,
  Download,
  ExternalLink,
  FileUp,
  History,
  KeyRound,
  Pencil,
  Plus,
  Power,
  PowerOff,
  RefreshCw,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useNavigate, useParams } from "react-router";
import { toast } from "sonner";
import type {
  CodexCredentialImportInput,
  CodexCredentialUpdateInput,
  CodexCredentialView,
  CodexQuotaResetOutcome,
  CodexQuotaWindowPeriod,
} from "@/api/types";
import { ApiError } from "@/api/errors";
import { translate, useI18n } from "@/app/i18n";
import { AsyncResource, ErrorAlert } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { EmptyState } from "@/components/shared/empty-state";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  useChannelGroup,
  useControlPlaneLists,
} from "@/features/admin/api";
import { formatDateTime } from "@/lib/dates";
import { formatEstimatedQuotaTotal, formatUsd } from "@/lib/formatters";
import {
  useBatchUpdateCodexCredentials,
  useCodexCredential,
  useCodexCredentials,
  useCodexQuotaWindowHistory,
  useCompleteCodexOauth,
  useDeleteCodexCredential,
  useExportCodexCredentials,
  useImportCodexCredential,
  useRefreshCodexCredential,
  useRefreshCodexQuota,
  useResetCodexQuota,
  useStartCodexOauth,
  useUpdateCodexCredential,
} from "./api";

const NO_PROXY = "__none__";
const MAX_BATCH_SIZE = 100;

function errorMessage(error: unknown): string {
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

interface CredentialSettingsState {
  label: string;
  proxy_id: string;
  weight: string;
  quota_threshold_percent: string;
}

interface ImportState extends CredentialSettingsState {
  id_token: string;
  access_token: string;
  refresh_token: string;
  account_id: string;
  user_id: string;
}

interface EditState extends CredentialSettingsState {
  enabled: boolean;
}

const EMPTY_SETTINGS: CredentialSettingsState = {
  label: "",
  proxy_id: "",
  weight: "100",
  quota_threshold_percent: "95",
};

const EMPTY_IMPORT: ImportState = {
  ...EMPTY_SETTINGS,
  id_token: "",
  access_token: "",
  refresh_token: "",
  account_id: "",
  user_id: "",
};

function parsePositiveInteger(value: string): number | null {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

function parseSettings(
  state: CredentialSettingsState,
): { label: string; proxy_id: string | null; weight: number; quota_threshold_percent: number } | null {
  const weight = parsePositiveInteger(state.weight);
  const threshold = parsePositiveInteger(state.quota_threshold_percent);
  if (
    !state.label.trim() ||
    weight === null ||
    threshold === null ||
    threshold > 100
  ) {
    return null;
  }
  return {
    label: state.label.trim(),
    proxy_id: state.proxy_id || null,
    weight,
    quota_threshold_percent: threshold,
  };
}

function credentialStatusVariant(
  status: CodexCredentialView["runtime_status"],
): "success" | "warning" | "destructive" | "default" {
  if (status === "active") return "success";
  if (status === "draining") return "warning";
  if (status === "unavailable") return "destructive";
  return "default";
}

function quotaSummary(credential: CodexCredentialView): string {
  if (credential.quota_limit_reached) return translate("Limit reached");
  if (credential.quota_allowed === false) return translate("Not allowed");
  if (credential.primary_used_percent !== null) {
    return translate("{percent}% used", {
      percent: credential.primary_used_percent,
    });
  }
  return translate("Not checked");
}

function credentialStatusLabel(
  status: CodexCredentialView["runtime_status"],
): string {
  if (status === "active") return translate("Active");
  if (status === "draining") return translate("Draining");
  if (status === "unavailable") return translate("Unavailable");
  return translate("Disabled");
}

function quotaResetReasonLabel(
  reason: CodexQuotaWindowPeriod["reset_reason"],
): string {
  if (reason === "natural") return translate("Natural reset");
  if (reason === "manual") return translate("Manual reset credit");
  if (reason === "openai_official") return translate("OpenAI official reset");
  return translate("Current period");
}

function quotaResetReasonVariant(
  reason: CodexQuotaWindowPeriod["reset_reason"],
): "success" | "warning" | "info" | "secondary" {
  if (reason === "manual") return "warning";
  if (reason === "openai_official") return "info";
  if (reason === "natural") return "secondary";
  return "success";
}

function quotaWindowStatisticsEnd(period: CodexQuotaWindowPeriod): string {
  if (period.ended_at) return period.ended_at;
  const scheduled = new Date(period.scheduled_reset_at).getTime();
  return new Date(Math.min(Date.now(), scheduled)).toISOString();
}

function quotaWindowGranularity(
  period: CodexQuotaWindowPeriod,
): "hour" | "day" {
  return period.window_seconds <= 31 * 24 * 60 * 60 ? "hour" : "day";
}

function quotaResetOutcomeMessage(outcome: CodexQuotaResetOutcome): string {
  if (outcome === "reset") return translate("OpenAI reset credit consumed.");
  if (outcome === "nothing_to_reset") {
    return translate("No active quota window needed a reset.");
  }
  if (outcome === "no_credit") {
    return translate("No OpenAI reset credit is available.");
  }
  return translate("This reset request was already redeemed.");
}

function QuotaWindow({
  label,
  percent,
  resetAt,
  costAmount,
}: {
  label: string;
  percent: number | null;
  resetAt: string | null;
  costAmount: string | null;
}) {
  const { t } = useI18n();
  return (
    <div className="min-w-40 space-y-1">
      <div className="flex justify-between gap-3 text-xs">
        <span>{t(label)}</span>
        <span>{percent === null ? "—" : `${percent}%`}</span>
      </div>
      <Progress value={percent ?? 0} />
      {resetAt ? (
        <p className="text-xs text-muted-foreground">
          {t("Resets {time}", { time: formatDateTime(resetAt) })}
        </p>
      ) : null}
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">{t("Period spend")}</span>
        <span className="font-medium tabular-nums">
          {formatUsd(costAmount)}
        </span>
      </div>
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">
          {t("Estimated total quota")}
        </span>
        <span className="font-medium tabular-nums">
          {formatEstimatedQuotaTotal(costAmount, percent)}
        </span>
      </div>
    </div>
  );
}

function ProxyField({
  value,
  onChange,
  proxies,
}: {
  value: string;
  onChange: (value: string) => void;
  proxies: Array<{ id: string; name: string }>;
}) {
  const { t } = useI18n();
  return (
    <Field>
      <FieldLabel>{t("Outbound proxy")}</FieldLabel>
      <Select
        value={value || NO_PROXY}
        onValueChange={(next) => onChange(next === NO_PROXY ? "" : next)}
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem value={NO_PROXY}>{t("Direct")}</SelectItem>
            {proxies.map((proxy) => (
              <SelectItem key={proxy.id} value={proxy.id}>
                {proxy.name}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
    </Field>
  );
}

function SettingsFields({
  state,
  onChange,
  proxies,
}: {
  state: CredentialSettingsState;
  onChange: (patch: Partial<CredentialSettingsState>) => void;
  proxies: Array<{ id: string; name: string }>;
}) {
  const { t } = useI18n();
  return (
    <FieldGroup>
      <Field>
        <FieldLabel htmlFor="codex-label">{t("Label")}</FieldLabel>
        <Input
          id="codex-label"
          value={state.label}
          onChange={(event) => onChange({ label: event.target.value })}
          placeholder={t("Personal Codex account")}
        />
      </Field>
      <ProxyField
        value={state.proxy_id}
        onChange={(proxy_id) => onChange({ proxy_id })}
        proxies={proxies}
      />
      <div className="grid gap-4 sm:grid-cols-2">
        <Field>
          <FieldLabel htmlFor="codex-weight">{t("Weight")}</FieldLabel>
          <Input
            id="codex-weight"
            type="number"
            min={1}
            value={state.weight}
            onChange={(event) => onChange({ weight: event.target.value })}
          />
        </Field>
        <Field>
          <FieldLabel htmlFor="codex-threshold">
            {t("Quota threshold (%)")}
          </FieldLabel>
          <Input
            id="codex-threshold"
            type="number"
            min={1}
            max={100}
            value={state.quota_threshold_percent}
            onChange={(event) =>
              onChange({ quota_threshold_percent: event.target.value })
            }
          />
          <FieldDescription>
            {t(
              "New requests stop using this credential at the threshold; existing sticky Responses sessions may continue.",
            )}
          </FieldDescription>
        </Field>
      </div>
    </FieldGroup>
  );
}

export default function CodexOauthPage() {
  const { t } = useI18n();
  const { id: groupId = "" } = useParams();
  const navigate = useNavigate();
  const group = useChannelGroup(groupId);
  const lists = useControlPlaneLists();
  const credentials = useCodexCredentials(groupId);
  const startOauth = useStartCodexOauth(groupId);
  const completeOauth = useCompleteCodexOauth(groupId);
  const importCredential = useImportCodexCredential(groupId);
  const exportCredentials = useExportCodexCredentials(groupId);
  const batchUpdate = useBatchUpdateCodexCredentials(groupId);
  const deleteCredential = useDeleteCodexCredential(groupId);
  const refreshCredential = useRefreshCodexCredential(groupId);
  const refreshQuota = useRefreshCodexQuota(groupId);
  const resetQuota = useResetCodexQuota(groupId);

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
      (lists.data?.proxies ?? []).map((proxy) => ({
        id: proxy.id,
        name: proxy.name,
      })),
    [lists.data?.proxies],
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
      toast.error(t("Enter a label, positive weight, and quota threshold from 1 to 100."));
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
        `codex-credentials-${safeFilename(
          group.data?.data.name ?? "export",
        )}-${new Date().toISOString().slice(0, 10)}.json`,
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

  const groupError =
    group.data && group.data.data.connector_kind !== "codex_oauth"
      ? new Error(t("This channel group is not a Codex OAuth connector."))
      : null;
  const managementEnabled =
    group.data?.data.connector_kind === "codex_oauth" && !group.error;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={group.data?.data.name ?? t("Codex OAuth")}
        description={t(
          "Connect ChatGPT Codex subscriptions, share credentials across Responses and Images channels, assign proxies, and monitor quota. Estimated total quota divides current period spend by the most recently provider-reported used percentage.",
        )}
        actions={
          <>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => navigate("/admin/routing/channels")}
            >
              <ArrowLeft data-icon="inline-start" />
              {t("Back to channels")}
            </Button>
            <Button
              variant="outline"
              disabled={!managementEnabled || exportCredentials.isPending}
              onClick={() => setExportIds([])}
            >
              <Download data-icon="inline-start" />
              {t("Export credentials")}
            </Button>
            <Button
              variant="outline"
              disabled={!managementEnabled}
              onClick={() =>
                navigate(`/admin/providers/codex-oauth/${groupId}/import`)
              }
            >
              <FileUp data-icon="inline-start" />
              {t("Advanced import")}
            </Button>
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

      {group.error ? <ErrorAlert error={group.error} /> : null}
      {groupError ? <ErrorAlert error={groupError} /> : null}

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

      <Card>
        <CardHeader>
          <CardTitle>{t("Managed credentials")}</CardTitle>
          <CardDescription>
            {t(
              "Each credential projects to separate Responses and Images managed channels. Existing sticky Responses sessions may continue while quota is draining.",
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
            <div className="overflow-x-auto rounded-xl border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-12">
                      <Checkbox
                        aria-label={t("Select all credentials")}
                        checked={allSelected}
                        onCheckedChange={() => {
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
                      />
                    </TableHead>
                    <TableHead>{t("Credential")}</TableHead>
                    <TableHead>{t("Status")}</TableHead>
                    <TableHead>{t("Quota")}</TableHead>
                    <TableHead>{t("Token")}</TableHead>
                    <TableHead>{t("Routing")}</TableHead>
                    <TableHead className="text-right">{t("Actions")}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {credentials.data?.map((credential) => (
                    <TableRow key={credential.id}>
                      <TableCell className="align-top">
                        <Checkbox
                          aria-label={t("Select {label}", {
                            label: credential.label,
                          })}
                          checked={selected.has(credential.id)}
                          onCheckedChange={() => toggleCredential(credential)}
                        />
                      </TableCell>
                      <TableCell className="min-w-52 align-top">
                        <div className="font-medium">{credential.label}</div>
                        <div className="text-xs text-muted-foreground">
                          {credential.email ??
                            credential.user_id ??
                            credential.account_id ??
                            t("Personal Codex account")}
                        </div>
                        {credential.account_id ? (
                          <p className="mt-1 break-all text-xs text-muted-foreground">
                            {t("Workspace {id}", { id: credential.account_id })}
                          </p>
                        ) : (
                          <p className="mt-1 text-xs text-muted-foreground">
                            {t("Personal credential (no workspace ID)")}
                          </p>
                        )}
                        {credential.user_id ? (
                          <p className="mt-1 break-all text-xs text-muted-foreground">
                            {t(
                              credential.account_id ? "Member {id}" : "User {id}",
                              { id: credential.user_id },
                            )}
                          </p>
                        ) : null}
                        <div className="mt-2 flex flex-wrap gap-1">
                          {credential.plan_type ? (
                            <Badge variant="secondary">{credential.plan_type}</Badge>
                          ) : null}
                          {credential.is_fedramp ? (
                            <Badge variant="outline">FedRAMP</Badge>
                          ) : null}
                        </div>
                        <p className="mt-2 text-xs text-muted-foreground">
                          {t("Responses models: {count}", {
                            count: credential.available_models.length,
                          })}
                        </p>
                        {credential.last_error_code ? (
                          <p className="mt-2 max-w-64 text-xs text-destructive">
                            {credential.last_error_code}
                            {credential.last_error_summary
                              ? `: ${credential.last_error_summary}`
                              : ""}
                          </p>
                        ) : null}
                      </TableCell>
                      <TableCell className="align-top">
                        <StatusBadge
                          value={credential.runtime_status}
                          label={credentialStatusLabel(credential.runtime_status)}
                          variant={credentialStatusVariant(
                            credential.runtime_status,
                          )}
                        />
                        {!credential.enabled ? (
                          <div className="mt-2">
                            <StatusBadge value={false} />
                          </div>
                        ) : null}
                      </TableCell>
                      <TableCell className="min-w-56 align-top">
                        <div className="mb-2 text-sm font-medium">
                          {quotaSummary(credential)}
                        </div>
                        <div className="space-y-3">
                          <QuotaWindow
                            label="Primary window"
                            percent={credential.primary_used_percent}
                            resetAt={credential.primary_reset_at}
                            costAmount={
                              credential.primary_window_cost_amount
                            }
                          />
                          {credential.secondary_used_percent !== null ? (
                            <QuotaWindow
                              label="Secondary window"
                              percent={credential.secondary_used_percent}
                              resetAt={credential.secondary_reset_at}
                              costAmount={
                                credential.secondary_window_cost_amount
                              }
                            />
                          ) : null}
                        </div>
                        <p className="mt-2 text-xs text-muted-foreground">
                          {t("Threshold: {percent}%", {
                            percent: credential.quota_threshold_percent,
                          })}
                        </p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {t("OpenAI reset credits: {count}", {
                            count:
                              credential.quota_reset_credits_available ?? "—",
                          })}
                        </p>
                      </TableCell>
                      <TableCell className="min-w-44 align-top text-sm">
                        <p>{formatDateTime(credential.access_token_expires_at)}</p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {t("Refreshed {time}", {
                            time: formatDateTime(credential.last_refreshed_at),
                          })}
                        </p>
                      </TableCell>
                      <TableCell className="align-top text-sm">
                        <p>{t("Weight {weight}", { weight: credential.weight })}</p>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {credential.proxy_id ? t("Proxy assigned") : t("Direct")}
                        </p>
                      </TableCell>
                      <TableCell className="align-top">
                        <div className="flex flex-col items-end gap-1">
                          <div className="flex gap-1">
                            <Tooltip disableHoverablePopup>
                              <TooltipTrigger
                                render={
                                  <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t("View quota history for {label}", {
                                      label: credential.label,
                                    })}
                                    onClick={() => setHistoryTarget(credential)}
                                  />
                                }
                              >
                                <History />
                              </TooltipTrigger>
                              <TooltipContent side="left">
                                {t("View quota history for {label}", {
                                  label: credential.label,
                                })}
                              </TooltipContent>
                            </Tooltip>
                            <Tooltip disableHoverablePopup>
                              <TooltipTrigger
                                render={
                                  <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t("Edit {label}", {
                                      label: credential.label,
                                    })}
                                    onClick={() => setEditingId(credential.id)}
                                  />
                                }
                              >
                                <Pencil />
                              </TooltipTrigger>
                              <TooltipContent side="left">
                                {t("Edit {label}", {
                                  label: credential.label,
                                })}
                              </TooltipContent>
                            </Tooltip>
                          </div>
                          <div className="flex gap-1">
                            <Tooltip disableHoverablePopup>
                              <TooltipTrigger
                                render={
                                  <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t(
                                      "Reset quota with an OpenAI credit for {label}",
                                      { label: credential.label },
                                    )}
                                    disabled={
                                      resetQuota.isPending ||
                                      credential.quota_reset_credits_available === 0
                                    }
                                    onClick={() => setResetTarget(credential)}
                                  />
                                }
                              >
                                <RotateCcw />
                              </TooltipTrigger>
                              <TooltipContent side="left">
                                {t(
                                  "Reset quota with an OpenAI credit for {label}",
                                  { label: credential.label },
                                )}
                              </TooltipContent>
                            </Tooltip>
                            <Tooltip disableHoverablePopup>
                              <TooltipTrigger
                                render={
                                  <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t("Refresh quota for {label}", {
                                      label: credential.label,
                                    })}
                                    disabled={refreshQuota.isPending}
                                    onClick={() => void runRefresh("quota", credential)}
                                  />
                                }
                              >
                                <RefreshCw className="text-info" />
                              </TooltipTrigger>
                              <TooltipContent side="left">
                                {t("Refresh quota for {label}", {
                                  label: credential.label,
                                })}
                              </TooltipContent>
                            </Tooltip>
                            <Tooltip disableHoverablePopup>
                              <TooltipTrigger
                                render={
                                  <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t("Refresh token for {label}", {
                                      label: credential.label,
                                    })}
                                    disabled={refreshCredential.isPending}
                                    onClick={() => void runRefresh("token", credential)}
                                  />
                                }
                              >
                                <RefreshCw />
                              </TooltipTrigger>
                              <TooltipContent side="left">
                                {t("Refresh token for {label}", {
                                  label: credential.label,
                                })}
                              </TooltipContent>
                            </Tooltip>
                            <Tooltip disableHoverablePopup>
                              <TooltipTrigger
                                render={
                                  <Button
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t("Delete {label}", {
                                      label: credential.label,
                                    })}
                                    disabled={deleteCredential.isPending}
                                    onClick={() => setDeleteTarget(credential)}
                                  />
                                }
                              >
                                <Trash2 />
                              </TooltipTrigger>
                              <TooltipContent side="left">
                                {t("Delete {label}", {
                                  label: credential.label,
                                })}
                              </TooltipContent>
                            </Tooltip>
                          </div>
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </AsyncResource>
        </CardContent>
      </Card>

      <Dialog open={oauthOpen} onOpenChange={(open) => (open ? setOauthOpen(true) : closeOauth())}>
        <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>{t("Connect Codex account")}</DialogTitle>
            <DialogDescription>
              {t(
                "The gateway creates a short-lived PKCE flow. Authorize in a browser, then paste the complete localhost callback URL.",
              )}
            </DialogDescription>
          </DialogHeader>
          {oauthFlow ? (
            <FieldGroup>
              <Alert>
                <ExternalLink />
                <AlertTitle>{t("Authorization ready")}</AlertTitle>
                <AlertDescription>
                  {t("This flow expires at {time}.", {
                    time: formatDateTime(oauthFlow.expiresAt),
                  })}
                </AlertDescription>
              </Alert>
              <Field>
                <FieldLabel htmlFor="codex-authorization-url">
                  {t("Authorization URL")}
                </FieldLabel>
                <Textarea
                  id="codex-authorization-url"
                  readOnly
                  value={oauthFlow.authorizationUrl}
                  rows={4}
                />
              </Field>
              <Button
                type="button"
                variant="outline"
                onClick={() =>
                  window.open(
                    oauthFlow.authorizationUrl,
                    "_blank",
                    "noopener,noreferrer",
                  )
                }
              >
                <ExternalLink data-icon="inline-start" />
                {t("Open authorization page")}
              </Button>
              <Field>
                <FieldLabel htmlFor="codex-callback-url">
                  {t("Callback URL")}
                </FieldLabel>
                <Textarea
                  id="codex-callback-url"
                  value={callbackUrl}
                  onChange={(event) => setCallbackUrl(event.target.value)}
                  placeholder="http://localhost:1455/auth/callback?code=…&state=…"
                  rows={4}
                />
              </Field>
            </FieldGroup>
          ) : (
            <SettingsFields
              state={oauthSettings}
              onChange={(patch) =>
                setOauthSettings((current) => ({ ...current, ...patch }))
              }
              proxies={proxies}
            />
          )}
          <DialogFooter>
            <Button variant="outline" onClick={closeOauth}>
              {t("Cancel")}
            </Button>
            <Button
              onClick={() => void (oauthFlow ? finishOauth() : beginOauth())}
              disabled={startOauth.isPending || completeOauth.isPending}
            >
              {oauthFlow ? t("Complete connection") : t("Start authorization")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={importOpen}
        onOpenChange={(open) => {
          setImportOpen(open);
          if (!open) setImportState(EMPTY_IMPORT);
        }}
      >
        <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>{t("Import Codex tokens")}</DialogTitle>
            <DialogDescription>
              {t(
                "Tokens are validated against Codex before format-specific managed channels are created. They are not returned by the Console API.",
              )}
            </DialogDescription>
          </DialogHeader>
          <SettingsFields
            state={importState}
            onChange={(patch) =>
              setImportState((current) => ({ ...current, ...patch }))
            }
            proxies={proxies}
          />
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="codex-id-token">
                {t("ID token (optional)")}
              </FieldLabel>
              <Input
                id="codex-id-token"
                type="password"
                autoComplete="off"
                value={importState.id_token}
                onChange={(event) =>
                  setImportState((current) => ({
                    ...current,
                    id_token: event.target.value,
                  }))
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-access-token">
                {t("Access token")}
              </FieldLabel>
              <Input
                id="codex-access-token"
                type="password"
                autoComplete="off"
                value={importState.access_token}
                onChange={(event) =>
                  setImportState((current) => ({
                    ...current,
                    access_token: event.target.value,
                  }))
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-refresh-token">
                {t("Refresh token")}
              </FieldLabel>
              <Input
                id="codex-refresh-token"
                type="password"
                autoComplete="off"
                value={importState.refresh_token}
                onChange={(event) =>
                  setImportState((current) => ({
                    ...current,
                    refresh_token: event.target.value,
                  }))
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-account-id">
                {t("Account ID override (optional)")}
              </FieldLabel>
              <Input
                id="codex-account-id"
                value={importState.account_id}
                onChange={(event) =>
                  setImportState((current) => ({
                    ...current,
                    account_id: event.target.value,
                  }))
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-user-id">
                {t("User ID override (optional)")}
              </FieldLabel>
              <Input
                id="codex-user-id"
                value={importState.user_id}
                onChange={(event) =>
                  setImportState((current) => ({
                    ...current,
                    user_id: event.target.value,
                  }))
                }
              />
            </Field>
          </FieldGroup>
          <DialogFooter>
            <Button variant="outline" onClick={() => setImportOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button
              onClick={() => void importTokens()}
              disabled={importCredential.isPending}
            >
              {t("Validate and import")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <EditCredentialDialog
        groupId={groupId}
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

function QuotaWindowHistoryDialog({
  credential,
  onClose,
}: {
  credential: CodexCredentialView | null;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const history = useCodexQuotaWindowHistory(credential?.id ?? "");
  const primary = (history.data?.periods ?? []).filter(
    (period) => period.window_kind === "primary",
  );
  const secondary = (history.data?.periods ?? []).filter(
    (period) => period.window_kind === "secondary",
  );

  const viewCosts = (period: CodexQuotaWindowPeriod) => {
    if (!credential) return;
    const endedAt = quotaWindowStatisticsEnd(period);
    if (new Date(endedAt).getTime() <= new Date(period.started_at).getTime()) {
      toast.error(t("This quota period has no elapsed time to analyze."));
      return;
    }
    const search = new URLSearchParams({
      started_after: period.started_at,
      started_before: endedAt,
      granularity: quotaWindowGranularity(period),
      codex_credential_id: credential.id,
    });
    onClose();
    navigate(`/admin/cost-statistics?${search.toString()}`);
  };

  return (
    <Dialog open={credential !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-5xl">
        <DialogHeader>
          <DialogTitle>
            {t("Quota window history for {label}", {
              label: credential?.label ?? "",
            })}
          </DialogTitle>
          <DialogDescription>
            {t(
              "Natural resets follow the scheduled boundary. Manual resets consume a reset credit through this Console. An earlier unmatched rollover is recorded as an OpenAI official reset. Period spend includes every caller and both Responses and Images projections.",
            )}
          </DialogDescription>
        </DialogHeader>
        <AsyncResource
          isLoading={history.isLoading}
          error={history.error}
          isEmpty={(history.data?.periods.length ?? 0) === 0}
          emptyTitle={t("No quota window history")}
          emptyDescription={t("History begins with the first stored quota observation.")}
        >
          <Tabs defaultValue="primary">
            <TabsList>
              <TabsTrigger value="primary">
                {t("Primary window")} ({primary.length})
              </TabsTrigger>
              <TabsTrigger value="secondary">
                {t("Secondary window")} ({secondary.length})
              </TabsTrigger>
            </TabsList>
            <TabsContent value="primary">
              <QuotaWindowPeriodsTable periods={primary} onViewCosts={viewCosts} />
            </TabsContent>
            <TabsContent value="secondary">
              <QuotaWindowPeriodsTable periods={secondary} onViewCosts={viewCosts} />
            </TabsContent>
          </Tabs>
        </AsyncResource>
      </DialogContent>
    </Dialog>
  );
}

function QuotaWindowPeriodsTable({
  periods,
  onViewCosts,
}: {
  periods: CodexQuotaWindowPeriod[];
  onViewCosts: (period: CodexQuotaWindowPeriod) => void;
}) {
  const { t } = useI18n();
  if (periods.length === 0) {
    return (
      <EmptyState
        title={t("No periods for this window")}
        description={t("The provider has not reported this quota window yet.")}
        className="py-10"
      />
    );
  }
  return (
    <div className="overflow-x-auto rounded-xl border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{t("Period")}</TableHead>
            <TableHead>{t("Usage")}</TableHead>
            <TableHead>{t("Period spend")}</TableHead>
            <TableHead>{t("Estimated total quota")}</TableHead>
            <TableHead>{t("Ended by")}</TableHead>
            <TableHead>{t("Last observed")}</TableHead>
            <TableHead className="text-right">{t("Actions")}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {periods.map((period) => (
            <TableRow key={period.id}>
              <TableCell className="min-w-64 align-top">
                <div className="flex flex-col gap-1">
                  <span className="font-medium">
                    {formatDateTime(period.started_at)}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {period.ended_at
                      ? t("Ended {time}", {
                          time: formatDateTime(period.ended_at),
                        })
                      : t("Scheduled reset {time}", {
                          time: formatDateTime(period.scheduled_reset_at),
                        })}
                  </span>
                  <Badge variant="outline" className="w-fit">
                    {period.window_seconds % 86_400 === 0
                      ? `${period.window_seconds / 86_400}d`
                      : `${period.window_seconds / 3_600}h`}
                  </Badge>
                </div>
              </TableCell>
              <TableCell className="align-top tabular-nums">
                {period.initial_used_percent}% → {period.last_used_percent}%
              </TableCell>
              <TableCell className="align-top tabular-nums">
                {formatUsd(period.cost_amount)}
              </TableCell>
              <TableCell className="align-top tabular-nums">
                {formatEstimatedQuotaTotal(
                  period.cost_amount,
                  period.last_used_percent,
                )}
              </TableCell>
              <TableCell className="align-top">
                <Badge variant={quotaResetReasonVariant(period.reset_reason)}>
                  {quotaResetReasonLabel(period.reset_reason)}
                </Badge>
              </TableCell>
              <TableCell className="align-top">
                {formatDateTime(period.last_observed_at)}
              </TableCell>
              <TableCell className="align-top text-right">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => onViewCosts(period)}
                >
                  <ExternalLink data-icon="inline-start" />
                  {t("View costs")}
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

function EditCredentialDialog({
  groupId,
  credentialId,
  proxies,
  onClose,
}: {
  groupId: string;
  credentialId: string | null;
  proxies: Array<{ id: string; name: string }>;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const detail = useCodexCredential(credentialId ?? "");
  const update = useUpdateCodexCredential(groupId, credentialId ?? "");
  const [state, setState] = useState<EditState>({
    ...EMPTY_SETTINGS,
    enabled: true,
  });

  useEffect(() => {
    if (!detail.data) return;
    const credential = detail.data.data;
    setState({
      label: credential.label,
      enabled: credential.enabled,
      proxy_id: credential.proxy_id ?? "",
      weight: String(credential.weight),
      quota_threshold_percent: String(credential.quota_threshold_percent),
    });
  }, [detail.data]);

  const save = async () => {
    const settings = parseSettings(state);
    if (!settings || !detail.etag) {
      toast.error(t("Enter valid credential settings."));
      return;
    }
    const input: CodexCredentialUpdateInput = {
      ...settings,
      enabled: state.enabled,
    };
    try {
      await update.mutateAsync({ input, ifMatch: detail.etag });
      toast.success(t("Credential updated."));
      onClose();
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("Credential changed elsewhere. Reloading."));
        await detail.refetch();
        return;
      }
      toast.error(errorMessage(error));
    }
  };

  return (
    <Dialog open={Boolean(credentialId)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t("Edit Codex credential")}</DialogTitle>
          <DialogDescription>
            {t("Change routing settings without exposing stored OAuth tokens.")}
          </DialogDescription>
        </DialogHeader>
        <AsyncResource
          isLoading={detail.isLoading}
          error={detail.error}
        >
          <FieldGroup>
            <Field orientation="horizontal">
              <FieldLabel htmlFor="codex-enabled">{t("Enabled")}</FieldLabel>
              <Switch
                id="codex-enabled"
                checked={state.enabled}
                onCheckedChange={(enabled) =>
                  setState((current) => ({ ...current, enabled }))
                }
              />
            </Field>
            <SettingsFields
              state={state}
              onChange={(patch) =>
                setState((current) => ({ ...current, ...patch }))
              }
              proxies={proxies}
            />
          </FieldGroup>
        </AsyncResource>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("Cancel")}
          </Button>
          <Button
            onClick={() => void save()}
            disabled={detail.isLoading || update.isPending}
          >
            {t("Save changes")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function safeFilename(value: string): string {
  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return normalized || "export";
}

function downloadJson(value: unknown, filename: string) {
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
