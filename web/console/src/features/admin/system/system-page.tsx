import { useEffect, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { toast } from "sonner";
import { RefreshCw, Save } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { DetailField } from "@/components/shared/detail-field";
import { ApiError } from "@/api/errors";
import { useReload, useSystemSettings, useUpdateSystemSettings } from "@/features/admin/api";
import { useI18n } from "@/app/i18n";

const systemSettingsSchema = z
  .object({
    upstream: z.object({
      connect_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
      response_header_timeout_seconds: z
        .number()
        .int()
        .min(1, "Enter a positive number of seconds."),
      stream_idle_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
    }),
    passive_health: z.object({
      connection_failure_threshold: z
        .number()
        .int()
        .min(1, "Enter a positive failure threshold."),
      cooldown_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
    }),
  })
  .superRefine((value, context) => {
    if (value.upstream.response_header_timeout_seconds <= value.upstream.connect_timeout_seconds) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["upstream", "response_header_timeout_seconds"],
        message: "Response header timeout must exceed connect timeout.",
      });
    }
  });

type SystemSettingsValues = z.infer<typeof systemSettingsSchema>;

const defaultValues: SystemSettingsValues = {
  upstream: {
    connect_timeout_seconds: 10,
    response_header_timeout_seconds: 30,
    stream_idle_timeout_seconds: 90,
  },
  passive_health: {
    connection_failure_threshold: 3,
    cooldown_seconds: 30,
  },
};

export function SystemPage() {
  const { t } = useI18n();
  const settings = useSystemSettings();
  const updateSettings = useUpdateSystemSettings();
  const reload = useReload();
  const [correlation, setCorrelation] = useState<string | null>(null);
  const form = useForm<SystemSettingsValues>({
    resolver: zodResolver(systemSettingsSchema),
    defaultValues,
  });

  useEffect(() => {
    if (settings.data) {
      form.reset({
        upstream: settings.data.data.upstream,
        passive_health: settings.data.data.passive_health,
      });
    }
  }, [form, settings.data]);

  const save = async (values: SystemSettingsValues) => {
    if (!settings.data) return;
    try {
      const result = await updateSettings.mutateAsync({
        input: values,
        ifMatch: settings.data.etag,
      });
      setCorrelation(result.correlation_id);
      toast.success(t("System settings saved and applied."));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("System settings changed elsewhere. Reloading."));
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
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("System settings")}
        description={t("Database-backed forwarding defaults for future requests.")}
      />
      <Alert>
        <AlertTitle>{t("Applies immediately")}</AlertTitle>
        <AlertDescription>
          {t(
            "Saving validates the full routing configuration and publishes a new runtime snapshot. Requests already in flight retain their original settings.",
          )}
        </AlertDescription>
      </Alert>
      <AsyncResource isLoading={settings.isLoading} error={settings.error}>
        {settings.data ? (
          <form onSubmit={form.handleSubmit(save)} className="flex flex-col gap-6">
            <Card>
              <CardHeader>
                <CardTitle>{t("Default upstream timeouts")}</CardTitle>
                <CardDescription>
                  {t(
                    "Used only when a channel does not define an explicit timeout. Response header timeout must be greater than connect timeout.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field data-invalid={Boolean(form.formState.errors.upstream?.connect_timeout_seconds)}>
                    <FieldLabel htmlFor="connect_timeout_seconds">
                      {t("Connect timeout (seconds)")}
                    </FieldLabel>
                    <Input
                      id="connect_timeout_seconds"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(form.formState.errors.upstream?.connect_timeout_seconds)}
                      {...form.register("upstream.connect_timeout_seconds", { valueAsNumber: true })}
                    />
                    {form.formState.errors.upstream?.connect_timeout_seconds ? (
                      <FieldError>
                        {form.formState.errors.upstream.connect_timeout_seconds.message}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.upstream?.response_header_timeout_seconds,
                    )}
                  >
                    <FieldLabel htmlFor="response_header_timeout_seconds">
                      {t("Response header timeout (seconds)")}
                    </FieldLabel>
                    <Input
                      id="response_header_timeout_seconds"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.upstream?.response_header_timeout_seconds,
                      )}
                      {...form.register("upstream.response_header_timeout_seconds", {
                        valueAsNumber: true,
                      })}
                    />
                    {form.formState.errors.upstream?.response_header_timeout_seconds ? (
                      <FieldError>
                        {form.formState.errors.upstream.response_header_timeout_seconds.message}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.upstream?.stream_idle_timeout_seconds)}>
                    <FieldLabel htmlFor="stream_idle_timeout_seconds">
                      {t("Stream idle timeout (seconds)")}
                    </FieldLabel>
                    <Input
                      id="stream_idle_timeout_seconds"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.upstream?.stream_idle_timeout_seconds,
                      )}
                      {...form.register("upstream.stream_idle_timeout_seconds", {
                        valueAsNumber: true,
                      })}
                    />
                    {form.formState.errors.upstream?.stream_idle_timeout_seconds ? (
                      <FieldError>
                        {form.formState.errors.upstream.stream_idle_timeout_seconds.message}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Passive health")}</CardTitle>
                <CardDescription>
                  {t(
                    "After the configured number of pre-header connection failures, a channel enters cooldown before one half-open probe is allowed.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.passive_health?.connection_failure_threshold,
                    )}
                  >
                    <FieldLabel htmlFor="connection_failure_threshold">
                      {t("Connection failure threshold")}
                    </FieldLabel>
                    <Input
                      id="connection_failure_threshold"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.passive_health?.connection_failure_threshold,
                      )}
                      {...form.register("passive_health.connection_failure_threshold", {
                        valueAsNumber: true,
                      })}
                    />
                    <FieldDescription>
                      {t("Only connection failures before upstream response headers count.")}
                    </FieldDescription>
                    {form.formState.errors.passive_health?.connection_failure_threshold ? (
                      <FieldError>
                        {form.formState.errors.passive_health.connection_failure_threshold.message}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.passive_health?.cooldown_seconds)}>
                    <FieldLabel htmlFor="cooldown_seconds">{t("Cooldown (seconds)")}</FieldLabel>
                    <Input
                      id="cooldown_seconds"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(form.formState.errors.passive_health?.cooldown_seconds)}
                      {...form.register("passive_health.cooldown_seconds", {
                        valueAsNumber: true,
                      })}
                    />
                    {form.formState.errors.passive_health?.cooldown_seconds ? (
                      <FieldError>
                        {form.formState.errors.passive_health.cooldown_seconds.message}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <Button type="submit" className="self-start" disabled={updateSettings.isPending}>
              {updateSettings.isPending ? (
                <Spinner data-icon="inline-start" />
              ) : (
                <Save data-icon="inline-start" />
              )}
              {t("Save system settings")}
            </Button>
          </form>
        ) : null}
      </AsyncResource>

      <Card>
        <CardHeader>
          <CardTitle>{t("Reload control plane")}</CardTitle>
          <CardDescription>
            {t(
              "Re-compiles and publishes the immutable runtime snapshot. Periodic reloads also run automatically.",
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
              <dl>
                <DetailField label={t("Correlation id")} value={correlation} mono />
              </dl>
            ) : null}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
