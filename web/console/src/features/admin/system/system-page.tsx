import { useCallback, useEffect, useId, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { useConfigurationDraft } from "@/hooks/use-configuration-draft";
import { z } from "zod";
import { toast } from "sonner";
import { RefreshCw, Save } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { FieldSet } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { PageHeader } from "@/components/shared/page-header";
import { DetailField } from "@/components/shared/detail-field";
import { ApiError } from "@/api/errors";
import { useReload, useSystemSettings, useUpdateSystemSettings } from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import type { SystemSettingsInput } from "@/api/types";
import type { SettingsSection } from "./settings-sections";
import {
  defaultValues,
  parseStatusCodes,
  sectionSettingsSchema,
  type SystemSettingsValues,
  type SystemSettingsVersion,
} from "./system-settings-schema";
import { SettingsChanges, SystemSettingsFormSections } from "./system-settings-sections";

export function SystemPage({ section, label }: { section: SettingsSection; label: string }) {
  const formId = useId();
  const { t } = useI18n();
  const settings = useSystemSettings();
  const updateSettings = useUpdateSystemSettings();
  const reload = useReload();
  const [correlation, setCorrelation] = useState<string | null>(null);
  const [baseline, setBaseline] = useState<SystemSettingsVersion>();
  const [conflict, setConflict] = useState(false);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [reloadingLatest, setReloadingLatest] = useState(false);
  const form = useForm<SystemSettingsValues>({
    resolver: (values, context, options) =>
      zodResolver(
        sectionSettingsSchema(section) as z.ZodType<
          SystemSettingsValues,
          SystemSettingsValues
        >,
      )(values, context, options),
    defaultValues,
  });
  const { reset, formState: { isDirty, isSubmitting } } = form;
  const reloadRequired = conflict || Boolean(baseline && settings.error);
  const fieldsLocked = isSubmitting || reloadingLatest;
  const writeBlocked = reloadRequired || settings.isFetching || updateSettings.isPending || reloadingLatest;
  const draft = useConfigurationDraft(fieldsLocked, isDirty);

  const loadVersion = useCallback((version: SystemSettingsVersion) => {
    const data = version.data;
    reset({
      api_hosts: data.api_hosts,
      upstream: data.upstream,
      request_retry: {
        ...data.request_retry,
        retryable_status_codes: data.request_retry.retryable_status_codes.join(", "),
      },
      passive_health: data.passive_health,
      automatic_disable: {
        ...data.automatic_disable,
        error_status_codes: data.automatic_disable.error_status_codes.join(", "),
      },
      scheduled_testing: data.scheduled_testing,
      session_affinity: data.session_affinity,
      websocket: data.websocket,
    });
    setBaseline(version);
  }, [reset]);
  useEffect(() => {
    if (settings.data && settings.data !== baseline && !isDirty && !writeBlocked && !isSubmitting) {
      loadVersion(settings.data);
    }
  }, [settings.data, baseline, isDirty, writeBlocked, isSubmitting, loadVersion]);

  const reloadLatest = async () => {
    setDiscardOpen(false);
    setReloadingLatest(true);
    try {
      const result = await settings.refetch();
      if (result.isSuccess) {
        loadVersion(result.data);
        setConflict(false);
        draft.markSaved();
      }
    } finally {
      setReloadingLatest(false);
    }
  };

  const errorMessage = (message: string | undefined) => (message ? t(message) : undefined);

  const save = async (values: SystemSettingsValues) => {
    if (!baseline || writeBlocked) return;
    try {
      const input: SystemSettingsInput = {
        api_hosts: values.api_hosts,
        upstream: values.upstream,
        request_retry: {
          enabled: values.request_retry.enabled,
          max_retries: values.request_retry.max_retries,
          retryable_status_codes: parseStatusCodes(
            values.request_retry.retryable_status_codes,
          ),
        },
        passive_health: values.passive_health,
        automatic_disable: {
          enabled: values.automatic_disable.enabled,
          error_status_codes: parseStatusCodes(values.automatic_disable.error_status_codes),
          error_message_keywords: values.automatic_disable.error_message_keywords,
        },
        scheduled_testing: values.scheduled_testing,
        session_affinity: values.session_affinity,
        websocket: values.websocket,
      };
      await updateSettings.mutateAsync({
        input,
        ifMatch: baseline.etag,
      });
      form.reset(values);
      draft.markSaved();
      toast.success(t("System settings saved and applied."));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        setConflict(true);
      } else {
        toast.error(error instanceof Error ? error.message : t("Save failed"));
      }
    }
  };

  const run = async () => {
    try {
      const result = await reload.mutateAsync();
      setCorrelation(result.correlation_id);
      toast.success(t("Control plane reloaded"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Reload failed"));
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {draft.navigationGuard}
      <PageHeader
        title={label}
        actions={section !== "maintenance" && baseline && <Button type="submit" form={formId} disabled={fieldsLocked || writeBlocked}>
          {updateSettings.isPending ? <Spinner data-icon="inline-start" /> : <Save data-icon="inline-start" />}
          {t("Save system settings")}
        </Button>}
      />
      <AsyncResource
        isLoading={settings.isLoading}
        error={baseline && section !== "maintenance" ? null : settings.error}
      >
        {baseline && section !== "maintenance" ? (
          <form
            id={formId}
            data-slot="system-settings-columns"
            onSubmit={form.handleSubmit(save)}
            className="flex flex-col gap-6"
          >
            {reloadRequired ? (
              <Alert variant="destructive">
                {conflict ? (
                  <AlertTitle>{t("System settings changed elsewhere. Your draft was not saved.")}</AlertTitle>
                ) : null}
                <AlertDescription>
                  {settings.error
                    ? t("The latest version could not be loaded. Your draft has been kept.")
                    : t("Load the latest version before editing again. This will discard your draft.")}
                </AlertDescription>
                <Button
                  type="button"
                  variant="outline"
                  className="justify-self-start"
                  disabled={fieldsLocked || settings.isFetching}
                  onClick={() => isDirty ? setDiscardOpen(true) : void reloadLatest()}
                >
                  {reloadingLatest ? <Spinner data-icon="inline-start" /> : null}
                  {t("Load latest version")}
                </Button>
              </Alert>
            ) : null}
            <div className="grid min-w-0 items-start gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(14rem,.35fr)]">
            <FieldSet disabled={fieldsLocked} className="min-w-0 gap-3">
            <SystemSettingsFormSections section={section} form={form} fieldsLocked={fieldsLocked} errorMessage={errorMessage} />
            </FieldSet>
            {isDirty && <SettingsChanges control={form.control} />}
            </div>
          </form>
        ) : null}
      </AsyncResource>

      <ConfirmDialog
        open={discardOpen}
        onOpenChange={setDiscardOpen}
        title={t("Discard unsaved changes?")}
        description={t("Load the latest version before editing again. This will discard your draft.")}
        confirmLabel={t("Discard changes")}
        destructive
        onConfirm={() => void reloadLatest()}
      />

      {section === "maintenance" ? <Card variant="section">
        <CardHeader>
          <CardTitle>{t("Reload control plane")}</CardTitle>
          <CardDescription>
            {t(
              "Apply the current configuration now instead of waiting for the next automatic reload.",
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex flex-col gap-4">
            <Button type="button" className="self-start" onClick={run} disabled={reload.isPending}>
              {reload.isPending ? <Spinner data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}
              {t("Reload now")}
            </Button>
            {correlation ? (
              <details className="text-sm"><summary>{t("Operation details")}</summary>
                <dl className="mt-2"><DetailField label={t("Correlation id")} value={correlation} mono /></dl>
              </details>
            ) : null}
          </div>
        </CardContent>
      </Card> : null}
    </div>
  );
}
