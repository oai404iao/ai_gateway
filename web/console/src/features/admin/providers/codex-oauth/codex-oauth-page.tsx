import { useEffect, useMemo, useState } from "react";
import { ExternalLink, KeyRound, Pencil, Plus, RefreshCw } from "lucide-react";
import { useParams } from "react-router";
import { toast } from "sonner";
import type {
  CodexCredentialImportInput,
  CodexCredentialUpdateInput,
  CodexCredentialView,
} from "@/api/types";
import { ApiError } from "@/api/errors";
import { translate, useI18n } from "@/app/i18n";
import { AsyncResource, ErrorAlert } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
  useChannelGroup,
  useControlPlaneLists,
} from "@/features/admin/api";
import { formatDateTime } from "@/lib/dates";
import {
  useCodexCredential,
  useCodexCredentials,
  useCompleteCodexOauth,
  useImportCodexCredential,
  useRefreshCodexCredential,
  useRefreshCodexQuota,
  useStartCodexOauth,
  useUpdateCodexCredential,
} from "./api";

const NO_PROXY = "__none__";

function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.isConflict) {
      return translate("This Codex account is already connected or changed elsewhere.");
    }
    switch (error.code) {
      case "codex_oauth_state_mismatch":
        return translate("The OAuth callback state did not match. Start a new authorization.");
      case "codex_oauth_flow_expired":
        return translate("The OAuth flow expired. Start a new authorization.");
      case "codex_refresh_token_invalid":
        return translate("The refresh token is no longer valid. Connect the account again.");
      case "codex_account_changed":
        return translate("The refreshed token belongs to a different Codex account.");
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

function QuotaWindow({
  label,
  percent,
  resetAt,
}: {
  label: string;
  percent: number | null;
  resetAt: string | null;
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
            {t("New sessions stop using this credential at the threshold.")}
          </FieldDescription>
        </Field>
      </div>
    </FieldGroup>
  );
}

export default function CodexOauthPage() {
  const { t } = useI18n();
  const { id: groupId = "" } = useParams();
  const group = useChannelGroup(groupId);
  const lists = useControlPlaneLists();
  const credentials = useCodexCredentials(groupId);
  const startOauth = useStartCodexOauth(groupId);
  const completeOauth = useCompleteCodexOauth(groupId);
  const importCredential = useImportCodexCredential(groupId);
  const refreshCredential = useRefreshCodexCredential(groupId);
  const refreshQuota = useRefreshCodexQuota(groupId);

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
  const [importState, setImportState] = useState<ImportState>(EMPTY_IMPORT);
  const [editingId, setEditingId] = useState<string | null>(null);

  const proxies = useMemo(
    () =>
      (lists.data?.proxies ?? []).map((proxy) => ({
        id: proxy.id,
        name: proxy.name,
      })),
    [lists.data?.proxies],
  );

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
      window.open(result.authorization_url, "_blank", "noopener,noreferrer");
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
      !importState.id_token.trim() ||
      !importState.access_token.trim() ||
      !importState.refresh_token.trim()
    ) {
      toast.error(t("Complete all required credential fields."));
      return;
    }
    const input: CodexCredentialImportInput = {
      ...settings,
      id_token: importState.id_token.trim(),
      access_token: importState.access_token.trim(),
      refresh_token: importState.refresh_token.trim(),
      account_id: importState.account_id.trim() || null,
    };
    try {
      await importCredential.mutateAsync(input);
      toast.success(t("Codex credential imported."));
      setImportOpen(false);
      setImportState(EMPTY_IMPORT);
    } catch (error) {
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
          "Connect ChatGPT Codex subscriptions, assign per-account proxies, and monitor quota.",
        )}
        actions={
          <>
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
            <CardDescription>{t("Ready for new sessions")}</CardDescription>
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
              "Each credential is a provider-managed channel. Existing sticky sessions may continue while quota is draining.",
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
            <div className="overflow-x-auto rounded-xl border">
              <Table>
                <TableHeader>
                  <TableRow>
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
                      <TableCell className="min-w-52 align-top">
                        <div className="font-medium">{credential.label}</div>
                        <div className="text-xs text-muted-foreground">
                          {credential.email ?? credential.account_id}
                        </div>
                        <div className="mt-2 flex flex-wrap gap-1">
                          {credential.plan_type ? (
                            <Badge variant="secondary">{credential.plan_type}</Badge>
                          ) : null}
                          {credential.is_fedramp ? (
                            <Badge variant="outline">FedRAMP</Badge>
                          ) : null}
                        </div>
                        <p className="mt-2 text-xs text-muted-foreground">
                          {t("{count} available models", {
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
                          />
                          {credential.secondary_used_percent !== null ? (
                            <QuotaWindow
                              label="Secondary window"
                              percent={credential.secondary_used_percent}
                              resetAt={credential.secondary_reset_at}
                            />
                          ) : null}
                        </div>
                        <p className="mt-2 text-xs text-muted-foreground">
                          {t("Threshold: {percent}%", {
                            percent: credential.quota_threshold_percent,
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
                        <div className="flex justify-end gap-1">
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("Refresh token for {label}", {
                              label: credential.label,
                            })}
                            disabled={refreshCredential.isPending}
                            onClick={() => void runRefresh("token", credential)}
                          >
                            <RefreshCw />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("Refresh quota for {label}", {
                              label: credential.label,
                            })}
                            disabled={refreshQuota.isPending}
                            onClick={() => void runRefresh("quota", credential)}
                          >
                            <RefreshCw className="text-info" />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("Edit {label}", {
                              label: credential.label,
                            })}
                            onClick={() => setEditingId(credential.id)}
                          >
                            <Pencil />
                          </Button>
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
                <AlertTitle>{t("Authorization opened")}</AlertTitle>
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
                "Tokens are validated against Codex before the managed channel is created. They are not returned by the Console API.",
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
              <FieldLabel htmlFor="codex-id-token">{t("ID token")}</FieldLabel>
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
