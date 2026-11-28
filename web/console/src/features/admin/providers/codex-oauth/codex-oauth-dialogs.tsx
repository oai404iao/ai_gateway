import { useEffect, useRef, useState } from "react";
import { ExternalLink } from "lucide-react";
import { toast } from "sonner";
import type {
  CodexCredentialUpdateInput,
  CodexCredentialView,
  CodexQuotaWindowPeriod,
} from "@/api/types";
import { ApiError } from "@/api/errors";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { formatDateTime } from "@/lib/dates";
import { formatEstimatedQuotaTotal, formatUsd } from "@/lib/formatters";
import {
  useCodexCredential,
  useCodexQuotaWindowHistory,
  useUpdateCodexCredential,
} from "./api";
import { SettingsFields } from "./codex-oauth-fields";
import {
  EMPTY_IMPORT,
  EMPTY_SETTINGS,
  errorMessage,
  parseSettings,
  quotaResetReasonLabel,
  quotaResetReasonVariant,
  type EditState,
  type ImportState,
  type CredentialSettingsState,
} from "./codex-oauth-helpers";

export interface ConnectOauthDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  oauthFlow: { authorizationUrl: string; expiresAt: string } | null;
  callbackUrl: string;
  onCallbackUrlChange: (value: string) => void;
  oauthSettings: CredentialSettingsState;
  onOauthSettingsChange: (patch: Partial<CredentialSettingsState>) => void;
  proxies: Array<{ id: string; name: string }>;
  isPending: boolean;
  onBegin: () => void;
  onFinish: () => void;
}

export function ConnectOauthDialog({
  open,
  onOpenChange,
  oauthFlow,
  callbackUrl,
  onCallbackUrlChange,
  oauthSettings,
  onOauthSettingsChange,
  proxies,
  isPending,
  onBegin,
  onFinish,
}: ConnectOauthDialogProps) {
  const { t } = useI18n();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t("Connect Codex account")}</DialogTitle>
          <DialogDescription>
            {t("After authorizing in your browser, paste the full localhost callback URL here.")}
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
                window.open(oauthFlow.authorizationUrl, "_blank", "noopener,noreferrer")
              }
            >
              <ExternalLink data-icon="inline-start" />
              {t("Open authorization page")}
            </Button>
            <Field>
              <FieldLabel htmlFor="codex-callback-url">{t("Callback URL")}</FieldLabel>
              <Textarea
                id="codex-callback-url"
                value={callbackUrl}
                onChange={(event) => onCallbackUrlChange(event.target.value)}
                placeholder="http://localhost:1455/auth/callback?code=…&state=…"
                rows={4}
              />
            </Field>
          </FieldGroup>
        ) : (
          <SettingsFields
            state={oauthSettings}
            onChange={onOauthSettingsChange}
            proxies={proxies}
          />
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button
            onClick={() => void (oauthFlow ? onFinish() : onBegin())}
            disabled={isPending}
          >
            {oauthFlow ? t("Complete connection") : t("Start authorization")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export interface ImportTokensDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  importState: ImportState;
  onImportStateChange: (patch: Partial<ImportState>) => void;
  proxies: Array<{ id: string; name: string }>;
  isPending: boolean;
  onImport: () => void;
}

export function ImportTokensDialog({
  open,
  onOpenChange,
  importState,
  onImportStateChange,
  proxies,
  isPending,
  onImport,
}: ImportTokensDialogProps) {
  const { t } = useI18n();
  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        onOpenChange(nextOpen);
        if (!nextOpen) onImportStateChange(EMPTY_IMPORT);
      }}
    >
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t("Import Codex tokens")}</DialogTitle>
          <DialogDescription>
            {t(
              "Import validates the tokens with Codex. Assign new credentials to a channel before use.",
            )}
          </DialogDescription>
        </DialogHeader>
        <SettingsFields
          state={importState}
          onChange={onImportStateChange}
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
              onChange={(event) => onImportStateChange({ id_token: event.target.value })}
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
              onChange={(event) => onImportStateChange({ access_token: event.target.value })}
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
              onChange={(event) => onImportStateChange({ refresh_token: event.target.value })}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="codex-account-id">
              {t("Account ID override (optional)")}
            </FieldLabel>
            <Input
              id="codex-account-id"
              value={importState.account_id}
              onChange={(event) => onImportStateChange({ account_id: event.target.value })}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="codex-user-id">
              {t("User ID override (optional)")}
            </FieldLabel>
            <Input
              id="codex-user-id"
              value={importState.user_id}
              onChange={(event) => onImportStateChange({ user_id: event.target.value })}
            />
          </Field>
        </FieldGroup>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button onClick={() => void onImport()} disabled={isPending}>
            {t("Validate and import")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function QuotaWindowHistoryDialog({
  credential,
  onClose,
}: {
  credential: CodexCredentialView | null;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const history = useCodexQuotaWindowHistory(credential?.id ?? "");
  const primary = (history.data?.periods ?? []).filter(
    (period) => period.window_kind === "primary",
  );
  const secondary = (history.data?.periods ?? []).filter(
    (period) => period.window_kind === "secondary",
  );

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
              <QuotaWindowPeriodsTable periods={primary} />
            </TabsContent>
            <TabsContent value="secondary">
              <QuotaWindowPeriodsTable periods={secondary} />
            </TabsContent>
          </Tabs>
        </AsyncResource>
      </DialogContent>
    </Dialog>
  );
}

function QuotaWindowPeriodsTable({
  periods,
}: {
  periods: CodexQuotaWindowPeriod[];
}) {
  const { t } = useI18n();
  const columns: Column<CodexQuotaWindowPeriod>[] = [
    {
      key: "period",
      header: t("Period"),
      className: "min-w-64 align-top",
      render: (period) => (
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
      ),
    },
    {
      key: "usage",
      header: t("Usage"),
      className: "align-top tabular-nums",
      render: (period) => `${period.initial_used_percent}% → ${period.last_used_percent}%`,
    },
    {
      key: "spend",
      header: t("Period spend"),
      className: "align-top tabular-nums",
      render: (period) => formatUsd(period.cost_amount),
    },
    {
      key: "estimated",
      header: t("Estimated total quota"),
      className: "align-top tabular-nums",
      render: (period) =>
        formatEstimatedQuotaTotal(
          period.cost_amount,
          period.last_used_percent,
        ),
    },
    {
      key: "ended-by",
      header: t("Ended by"),
      className: "align-top",
      render: (period) => (
        <Badge variant={quotaResetReasonVariant(period.reset_reason)}>
          {quotaResetReasonLabel(period.reset_reason)}
        </Badge>
      ),
    },
    {
      key: "last-observed",
      header: t("Last observed"),
      className: "align-top",
      render: (period) => formatDateTime(period.last_observed_at),
    },
  ];
  return (
    <ResourceTable
      columns={columns}
      rows={periods}
      rowKey={(period) => period.id}
      empty={
        <EmptyState
          title={t("No periods for this window")}
          description={t("The provider has not reported this quota window yet.")}
          className="py-10"
        />
      }
      pagination={false}
    />
  );
}

export function EditCredentialDialog({
  credentialId,
  proxies,
  onClose,
}: {
  credentialId: string | null;
  proxies: Array<{ id: string; name: string }>;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const detail = useCodexCredential(credentialId ?? "");
  const update = useUpdateCodexCredential(credentialId ?? "");
  const [state, setState] = useState<EditState>({
    ...EMPTY_SETTINGS,
    enabled: true,
  });
  const [dirty, setDirty] = useState(false);
  const baselineRef = useRef<typeof detail.data>(undefined);

  useEffect(() => {
    baselineRef.current = undefined;
    setDirty(false);
    if (!credentialId) {
      setState({ ...EMPTY_SETTINGS, enabled: true });
    }
  }, [credentialId]);

  useEffect(() => {
    if (detail.data && detail.data !== baselineRef.current && !dirty) {
      baselineRef.current = detail.data;
      const credential = detail.data.data;
      setState({
        label: credential.label,
        enabled: credential.enabled,
        proxy_id: credential.proxy_id ?? "",
        quota_threshold_percent: String(credential.quota_threshold_percent),
      });
    }
  }, [detail.data, dirty]);

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
      setDirty(false);
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
                onCheckedChange={(enabled) => {
                  setDirty(true);
                  setState((current) => ({ ...current, enabled }));
                }}
              />
            </Field>
            <SettingsFields
              state={state}
              onChange={(patch) => {
                setDirty(true);
                setState((current) => ({ ...current, ...patch }));
              }}
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
