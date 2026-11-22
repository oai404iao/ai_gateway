import { useCallback, useEffect, useId, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, useFormState, useWatch, get, type Control } from "react-hook-form";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";
import { z } from "zod";
import { toast } from "sonner";
import { RefreshCw, Save } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { PageHeader } from "@/components/shared/page-header";
import { DetailField } from "@/components/shared/detail-field";
import { StringListField } from "@/components/shared/string-list-field";
import { ApiError } from "@/api/errors";
import { useReload, useSystemSettings, useUpdateSystemSettings } from "@/features/admin/api";
import { SessionAffinityCard } from "@/features/admin/system/session-affinity-card";
import { useI18n } from "@/app/i18n";
import type { ScheduledTestingMode, SystemSettings, SystemSettingsInput } from "@/api/types";
import { settingsLabels } from "./settings-labels";
import type { SettingsSection } from "./settings-sections";

function statusCodesAreValid(value: string): boolean {
  const codes = value
    .split(/[\s,]+/)
    .map((code) => code.trim())
    .filter(Boolean);
  const parsed = codes.map(Number);
  return (
    parsed.every((code) => Number.isInteger(code) && code >= 100 && code <= 599) &&
    new Set(parsed).size === parsed.length
  );
}

function parseStatusCodes(value: string): number[] {
  return value
    .split(/[\s,]+/)
    .map((code) => code.trim())
    .filter(Boolean)
    .map(Number);
}

function retryStatusCodesAreValid(value: string): boolean {
  const codes = parseStatusCodes(value);
  return (
    codes.length <= 100 &&
    codes.every((code) => Number.isInteger(code) && code >= 400 && code <= 599) &&
    new Set(codes).size === codes.length
  );
}

const systemSettingsSchema = z
  .object({
    api_hosts: z
      .array(
        z
          .string()
          .trim()
          .min(1, "API host cannot be blank.")
          .max(2048, "API host must be at most 2048 characters.")
          .refine(
            (value) => {
              try {
                const url = new URL(value);
                return (
                  (url.protocol === "http:" || url.protocol === "https:") &&
                  Boolean(url.hostname) &&
                  !url.username &&
                  !url.password &&
                  !url.search &&
                  !url.hash
                );
              } catch {
                return false;
              }
            },
            "Enter a valid HTTP(S) API host.",
          ),
      )
      .max(32, "Configure at most 32 API hosts.")
      .refine(
        (hosts) => {
          const normalizedHosts = hosts.map((host) => {
            try {
              return new URL(host).toString();
            } catch {
              return host;
            }
          });
          return new Set(normalizedHosts).size === hosts.length;
        },
        "API hosts must be unique.",
      ),
    upstream: z.object({
      connect_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
      response_header_timeout_seconds: z
        .number()
        .int()
        .min(1, "Enter a positive number of seconds."),
      images_response_header_timeout_seconds: z
        .number()
        .int()
        .min(1, "Enter a positive number of seconds."),
      standalone_web_search_response_header_timeout_seconds: z
        .number()
        .int()
        .min(1, "Enter a positive number of seconds."),
      stream_idle_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
    }),
    request_retry: z.object({
      enabled: z.boolean(),
      max_retries: z
        .number()
        .int()
        .min(1, "Maximum retries must be between 1 and 10.")
        .max(10, "Maximum retries must be between 1 and 10."),
      retryable_status_codes: z
        .string()
        .refine(
          retryStatusCodesAreValid,
          "Enter at most 100 unique HTTP status codes from 400 through 599, separated by commas.",
        ),
    }),
    passive_health: z.object({
      connection_failure_threshold: z
        .number()
        .int()
        .min(1, "Enter a positive failure threshold."),
      cooldown_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
    }),
    automatic_disable: z.object({
      enabled: z.boolean(),
      error_status_codes: z
        .string()
        .refine(
          statusCodesAreValid,
          "Enter unique HTTP status codes from 100 through 599, separated by commas.",
        ),
      error_message_keywords: z
        .array(z.string().trim().min(1, "Keyword cannot be blank.").max(200))
        .refine(
          (keywords) => new Set(keywords.map((keyword) => keyword.toLocaleLowerCase())).size === keywords.length,
          "Error keywords must be unique.",
        ),
    }),
    scheduled_testing: z.object({
      mode: z.enum(["global", "failure_only"]),
      auto_recover: z.boolean(),
      interval_minutes: z.number().int().min(1, "Enter a positive number of minutes."),
      prompt: z.string().trim().min(1, "Test prompt is required.").max(4000),
    }),
    session_affinity: z.object({
      enabled: z.boolean(),
      max_entries: z
        .number()
        .int()
        .min(1, "Enter a positive cache capacity.")
        .max(1_000_000, "Cache capacity cannot exceed 1000000 entries."),
      default_ttl_seconds: z
        .number()
        .int()
        .min(1, "Enter a positive number of seconds.")
        .max(604_800, "Affinity TTL cannot exceed 604800 seconds."),
      rules: z
        .array(
          z.object({
            name: z.string().trim().min(1).max(64),
            enabled: z.boolean(),
            api_formats: z
              .array(z.enum(["open_ai_chat_completions", "open_ai_responses"]))
              .min(1),
            model_regex: z.array(z.string().min(1).max(256)).max(8),
            key_sources: z
              .array(
                z.discriminatedUnion("type", [
                  z.object({
                    type: z.literal("request_header"),
                    name: z.string().trim().min(1).max(256),
                  }),
                  z.object({
                    type: z.literal("json_pointer"),
                    pointer: z.string().trim().startsWith("/").max(256),
                  }),
                ]),
              )
              .min(1)
              .max(8),
            value_regex: z.string().min(1).max(256).nullable(),
            ttl_seconds: z.number().int().min(1).max(604_800).nullable(),
          }),
        )
        .max(64)
        .refine(
          (rules) =>
            new Set(rules.map((rule) => rule.name.trim().toLocaleLowerCase())).size ===
            rules.length,
          "Rule names must be unique.",
        ),
    }),
    websocket: z.object({
      enabled: z.boolean(),
      max_idle_connections: z
        .number()
        .int()
        .min(0, "Idle pool capacity must be between 0 and 4096.")
        .max(4096, "Idle pool capacity must be between 0 and 4096."),
      idle_timeout_seconds: z
        .number()
        .int()
        .min(1, "WebSocket idle timeout must be between 1 and 3600 seconds.")
        .max(3600, "WebSocket idle timeout must be between 1 and 3600 seconds."),
      max_connection_age_seconds: z
        .number()
        .int()
        .min(60, "Maximum WebSocket age must be between 60 and 3600 seconds.")
        .max(3600, "Maximum WebSocket age must be between 60 and 3600 seconds."),
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
    if (
      value.upstream.images_response_header_timeout_seconds <=
      value.upstream.connect_timeout_seconds
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["upstream", "images_response_header_timeout_seconds"],
        message: "Images response header timeout must exceed connect timeout.",
      });
    }
    if (
      value.upstream.standalone_web_search_response_header_timeout_seconds <=
      value.upstream.connect_timeout_seconds
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: [
          "upstream",
          "standalone_web_search_response_header_timeout_seconds",
        ],
        message: "Web search response header timeout must exceed connect timeout.",
      });
    }
    if (value.websocket.idle_timeout_seconds >= value.websocket.max_connection_age_seconds) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["websocket", "max_connection_age_seconds"],
        message: "Maximum WebSocket age must exceed the idle timeout.",
      });
    }
  });

type SystemSettingsValues = z.infer<typeof systemSettingsSchema>;

function SettingsChanges({ control }: { control: Control<SystemSettingsValues> }) {
  const { t } = useI18n();
  const values = useWatch({ control });
  const { defaultValues: original } = useFormState({ control });
  const changes = Object.entries(settingsLabels).flatMap(([field, label]) => {
    const before: unknown = get(original, field), after: unknown = get(values, field);
    return JSON.stringify(before) === JSON.stringify(after) ? [] : [{ label, before, after }];
  });
  if (!changes.length) return null;
  const display = (value: unknown) => typeof value === "boolean" ? t(value ? "Yes" : "No") : String(value);
  return <aside aria-label={t("Pending settings changes")} className="min-w-0 border-t bg-muted/25 px-3 py-3 lg:border-t-0 lg:border-l">
    <h2 className="text-sm font-semibold">{t("Pending settings changes")}</h2>
    <ul className="mt-2 grid divide-y text-xs">{changes.map(({ label, before, after }) =>
      <li key={label} className="grid gap-1 py-2"><span>{t(label)}</span>
        {(["number", "boolean"].includes(typeof before) && typeof after === typeof before) ?
          <span><del className="text-muted-foreground">{display(before)}</del> → <strong>{typeof after === "number" && !Number.isFinite(after) ? "—" : display(after)}</strong></span> : <span className="text-muted-foreground">{t("Modified")}</span>}
      </li>)}</ul>
    <p className="mt-2 text-xs text-muted-foreground">{t("Saving applies to new requests immediately.")}</p>
  </aside>;
}
type SystemSettingsVersion = { data: SystemSettings; etag: string };

const defaultValues: SystemSettingsValues = {
  api_hosts: [],
  upstream: {
    connect_timeout_seconds: 10,
    response_header_timeout_seconds: 30,
    images_response_header_timeout_seconds: 300,
    standalone_web_search_response_header_timeout_seconds: 300,
    stream_idle_timeout_seconds: 90,
  },
  request_retry: {
    enabled: true,
    max_retries: 1,
    retryable_status_codes: "",
  },
  passive_health: {
    connection_failure_threshold: 3,
    cooldown_seconds: 30,
  },
  automatic_disable: {
    enabled: false,
    error_status_codes: "",
    error_message_keywords: [],
  },
  scheduled_testing: {
    mode: "global",
    auto_recover: true,
    interval_minutes: 5,
    prompt: "reply '1'",
  },
  session_affinity: {
    enabled: false,
    max_entries: 100_000,
    default_ttl_seconds: 3_600,
    rules: [],
  },
  websocket: {
    enabled: false,
    max_idle_connections: 128,
    idle_timeout_seconds: 300,
    max_connection_age_seconds: 3300,
  },
};

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
    resolver: zodResolver(systemSettingsSchema),
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
            {section === "general" ? (
            <Card variant="section">
              <CardContent>
                <FieldGroup>
                  <StringListField
                    id="api_hosts"
                    label={t(settingsLabels["api_hosts"])}
                    value={form.watch("api_hosts")}
                    onChange={(value) =>
                      form.setValue("api_hosts", value, {
                        shouldDirty: true,
                        shouldValidate: true,
                      })
                    }
                    placeholder="https://api.example.com/v1"
                    description={t(
                      "One HTTP(S) base URL per line. Paths are allowed; credentials, query strings, and fragments are not.",
                    )}
                    error={errorMessage(form.formState.errors.api_hosts?.message)}
                  />
                </FieldGroup>
              </CardContent>
            </Card>
            ) : null}

            {section === "upstream" ? (
            <Card variant="section">
              <CardHeader>
                <CardDescription>
                  {t(
                    "Channel overrides take precedence. Header timeouts must exceed the connect timeout.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field data-invalid={Boolean(form.formState.errors.upstream?.connect_timeout_seconds)}>
                    <FieldLabel htmlFor="connect_timeout_seconds">
                      {t(settingsLabels["upstream.connect_timeout_seconds"])}
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
                        {errorMessage(form.formState.errors.upstream.connect_timeout_seconds.message)}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.upstream?.response_header_timeout_seconds,
                    )}
                  >
                    <FieldLabel htmlFor="response_header_timeout_seconds">
                      {t(settingsLabels["upstream.response_header_timeout_seconds"])}
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
                        {errorMessage(
                          form.formState.errors.upstream.response_header_timeout_seconds.message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.upstream?.images_response_header_timeout_seconds,
                    )}
                  >
                    <FieldLabel htmlFor="images_response_header_timeout_seconds">
                      {t(settingsLabels["upstream.images_response_header_timeout_seconds"])}
                    </FieldLabel>
                    <Input
                      id="images_response_header_timeout_seconds"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.upstream?.images_response_header_timeout_seconds,
                      )}
                      {...form.register("upstream.images_response_header_timeout_seconds", {
                        valueAsNumber: true,
                      })}
                    />
                    {form.formState.errors.upstream?.images_response_header_timeout_seconds ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.upstream.images_response_header_timeout_seconds
                            .message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.upstream
                        ?.standalone_web_search_response_header_timeout_seconds,
                    )}
                  >
                    <FieldLabel htmlFor="standalone_web_search_response_header_timeout_seconds">
                      {t(settingsLabels["upstream.standalone_web_search_response_header_timeout_seconds"])}
                    </FieldLabel>
                    <Input
                      id="standalone_web_search_response_header_timeout_seconds"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.upstream
                          ?.standalone_web_search_response_header_timeout_seconds,
                      )}
                      {...form.register(
                        "upstream.standalone_web_search_response_header_timeout_seconds",
                        { valueAsNumber: true },
                      )}
                    />
                    {form.formState.errors.upstream
                      ?.standalone_web_search_response_header_timeout_seconds ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.upstream
                            .standalone_web_search_response_header_timeout_seconds
                            .message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.upstream?.stream_idle_timeout_seconds)}>
                    <FieldLabel htmlFor="stream_idle_timeout_seconds">
                      {t(settingsLabels["upstream.stream_idle_timeout_seconds"])}
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
                        {errorMessage(form.formState.errors.upstream.stream_idle_timeout_seconds.message)}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>
            ) : null}

            {section === "reliability" ? (
            <Card variant="section">
              <CardHeader>
                <CardTitle>{t("Request failover")}</CardTitle>
                <CardDescription>
                  {t(
                    "Replayable requests can fail over after connection errors, response-header timeouts, or configured upstream status codes. A timed-out upstream may still process the original request.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="request_retry_enabled">
                        {t(settingsLabels["request_retry.enabled"])}
                      </FieldLabel>
                      <FieldDescription>
                        {t(
                          "Retries never reuse the same channel/model candidate. Another model on the same channel remains eligible.",
                        )}
                      </FieldDescription>
                    </FieldContent>
                    <Switch
                      id="request_retry_enabled"
                      disabled={fieldsLocked}
                      checked={form.watch("request_retry.enabled")}
                      onCheckedChange={(checked) =>
                        form.setValue("request_retry.enabled", Boolean(checked), {
                          shouldDirty: true,
                          shouldValidate: true,
                        })
                      }
                    />
                  </Field>
                  <Field
                    data-invalid={Boolean(form.formState.errors.request_retry?.max_retries)}
                  >
                    <FieldLabel htmlFor="request_retry_max_retries">
                      {t(settingsLabels["request_retry.max_retries"])}
                    </FieldLabel>
                    <Input
                      id="request_retry_max_retries"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.request_retry?.max_retries,
                      )}
                      {...form.register("request_retry.max_retries", {
                        valueAsNumber: true,
                      })}
                    />
                    <FieldDescription>
                      {t(
                        "Does not include the initial request. A value of 1 allows one automatic failover.",
                      )}
                    </FieldDescription>
                    {form.formState.errors.request_retry?.max_retries ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.request_retry.max_retries.message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.request_retry
                        ?.retryable_status_codes,
                    )}
                  >
                    <FieldLabel htmlFor="request_retry_status_codes">
                      {t(settingsLabels["request_retry.retryable_status_codes"])}
                    </FieldLabel>
                    <Input
                      id="request_retry_status_codes"
                      placeholder="429, 502, 503, 504"
                      aria-invalid={Boolean(
                        form.formState.errors.request_retry
                          ?.retryable_status_codes,
                      )}
                      {...form.register(
                        "request_retry.retryable_status_codes",
                      )}
                    />
                    <FieldDescription>
                      {t(
                        "Comma-separated 4xx/5xx responses to discard and retry before anything is sent to the client. Leave empty to retry transport failures only.",
                      )}
                    </FieldDescription>
                    {form.formState.errors.request_retry
                      ?.retryable_status_codes ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.request_retry
                            .retryable_status_codes.message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>
            ) : null}

            {section === "reliability" ? (
            <Card variant="section">
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
                      {t(settingsLabels["passive_health.connection_failure_threshold"])}
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
                    {form.formState.errors.passive_health?.connection_failure_threshold ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.passive_health.connection_failure_threshold.message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.passive_health?.cooldown_seconds)}>
                    <FieldLabel htmlFor="cooldown_seconds">{t(settingsLabels["passive_health.cooldown_seconds"])}</FieldLabel>
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
                        {errorMessage(form.formState.errors.passive_health.cooldown_seconds.message)}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>
            ) : null}

            {section === "reliability" ? (
            <Card variant="section">
              <CardHeader>
                <CardTitle>{t("Automatic channel disable")}</CardTitle>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="automatic_disable_enabled">
                        {t(settingsLabels["automatic_disable.enabled"])}
                      </FieldLabel>
                    </FieldContent>
                    <Switch
                      id="automatic_disable_enabled"
                      disabled={fieldsLocked}
                      checked={form.watch("automatic_disable.enabled")}
                      onCheckedChange={(checked) =>
                        form.setValue("automatic_disable.enabled", Boolean(checked), {
                          shouldDirty: true,
                          shouldValidate: true,
                        })
                      }
                    />
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.automatic_disable?.error_status_codes,
                    )}
                  >
                    <FieldLabel htmlFor="automatic_disable_error_status_codes">
                      {t(settingsLabels["automatic_disable.error_status_codes"])}
                    </FieldLabel>
                    <Input
                      id="automatic_disable_error_status_codes"
                      placeholder="401, 429, 500"
                      aria-invalid={Boolean(
                        form.formState.errors.automatic_disable?.error_status_codes,
                      )}
                      {...form.register("automatic_disable.error_status_codes")}
                    />
                    <FieldDescription>
                      {t(
                        "Comma-separated upstream HTTP statuses. Matching a configured status disables an opted-in channel.",
                      )}
                    </FieldDescription>
                    {form.formState.errors.automatic_disable?.error_status_codes ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.automatic_disable.error_status_codes.message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <StringListField
                    id="automatic_disable_error_message_keywords"
                    variant="tokens"
                    label={t(settingsLabels["automatic_disable.error_message_keywords"])}
                    value={form.watch("automatic_disable.error_message_keywords")}
                    onChange={(value) =>
                      form.setValue("automatic_disable.error_message_keywords", value, {
                        shouldDirty: true,
                        shouldValidate: true,
                      })
                    }
                    placeholder={t("Enter an error keyword")}
                    description={t(
                      "Case-insensitive upstream error-message substrings. Response bodies are inspected only in memory.",
                    )}
                    error={errorMessage(
                      form.formState.errors.automatic_disable?.error_message_keywords?.message,
                    )}
                  />
                </FieldGroup>
              </CardContent>
            </Card>
            ) : null}

            {section === "testing" ? (
            <Card variant="section">
              <CardHeader>
                <CardDescription>
                  {t(
                    "Tests use the channel’s test model and are billed to the system administrator key.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field>
                    <FieldLabel htmlFor="scheduled_testing_mode">{t(settingsLabels["scheduled_testing.mode"])}</FieldLabel>
                    <Select
                      value={form.watch("scheduled_testing.mode")}
                      onValueChange={(value) =>
                        form.setValue(
                          "scheduled_testing.mode",
                          value as ScheduledTestingMode,
                          { shouldDirty: true, shouldValidate: true },
                        )
                      }
                    >
                      <SelectTrigger id="scheduled_testing_mode">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value="global">{t("Global")}</SelectItem>
                          <SelectItem value="failure_only">{t("Failures only")}</SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldDescription>
                      {t(
                        "Global tests all enabled channels; failures only tests temporarily auto-disabled channels.",
                      )}
                    </FieldDescription>
                  </Field>
                  <Field
                    data-invalid={Boolean(
                      form.formState.errors.scheduled_testing?.interval_minutes,
                    )}
                  >
                    <FieldLabel htmlFor="scheduled_testing_interval_minutes">
                      {t(settingsLabels["scheduled_testing.interval_minutes"])}
                    </FieldLabel>
                    <Input
                      id="scheduled_testing_interval_minutes"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(
                        form.formState.errors.scheduled_testing?.interval_minutes,
                      )}
                      {...form.register("scheduled_testing.interval_minutes", {
                        valueAsNumber: true,
                      })}
                    />
                    {form.formState.errors.scheduled_testing?.interval_minutes ? (
                      <FieldError>
                        {errorMessage(
                          form.formState.errors.scheduled_testing.interval_minutes.message,
                        )}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field
                    data-invalid={Boolean(form.formState.errors.scheduled_testing?.prompt)}
                  >
                    <FieldLabel htmlFor="scheduled_testing_prompt">{t(settingsLabels["scheduled_testing.prompt"])}</FieldLabel>
                    <Textarea
                      id="scheduled_testing_prompt"
                      rows={3}
                      aria-invalid={Boolean(form.formState.errors.scheduled_testing?.prompt)}
                      {...form.register("scheduled_testing.prompt")}
                    />
                    {form.formState.errors.scheduled_testing?.prompt ? (
                      <FieldError>
                        {errorMessage(form.formState.errors.scheduled_testing.prompt.message)}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="scheduled_testing_auto_recover">
                        {t(settingsLabels["scheduled_testing.auto_recover"])}
                      </FieldLabel>
                    </FieldContent>
                    <Switch
                      id="scheduled_testing_auto_recover"
                      disabled={fieldsLocked}
                      checked={form.watch("scheduled_testing.auto_recover")}
                      onCheckedChange={(checked) =>
                        form.setValue("scheduled_testing.auto_recover", Boolean(checked), {
                          shouldDirty: true,
                          shouldValidate: true,
                        })
                      }
                    />
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>
            ) : null}


            {section === "websocket" ? <div>
              <Card variant="section">
                <CardHeader>
                  <CardDescription>
                    {t(
                      "WebSocket forwarding requires the system, user, and selected Responses channel to be enabled. Pool settings apply process-wide.",
                    )}
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <FieldGroup>
                    <Field orientation="horizontal">
                      <FieldContent>
                        <FieldLabel htmlFor="websocket_enabled">
                          {t(settingsLabels["websocket.enabled"])}
                        </FieldLabel>
                        <FieldDescription>
                          {t(
                            "Disabled systems reject new WebSocket upgrades and discard idle upstream connections.",
                          )}
                        </FieldDescription>
                      </FieldContent>
                      <Switch
                        id="websocket_enabled"
                        disabled={fieldsLocked}
                        checked={form.watch("websocket.enabled")}
                        onCheckedChange={(checked) =>
                          form.setValue("websocket.enabled", Boolean(checked), {
                            shouldDirty: true,
                            shouldValidate: true,
                          })
                        }
                      />
                    </Field>

                    <div className="grid gap-4 md:grid-cols-3">
                      <Field
                        data-invalid={Boolean(
                          form.formState.errors.websocket?.max_idle_connections,
                        )}
                      >
                        <FieldLabel htmlFor="websocket_max_idle_connections">
                          {t(settingsLabels["websocket.max_idle_connections"])}
                        </FieldLabel>
                        <Input
                          id="websocket_max_idle_connections"
                          type="number"
                          min={0}
                          max={4096}
                          aria-invalid={Boolean(
                            form.formState.errors.websocket?.max_idle_connections,
                          )}
                          {...form.register("websocket.max_idle_connections", {
                            valueAsNumber: true,
                          })}
                        />
                        <FieldDescription>
                          {t("Set to zero to disable upstream connection reuse.")}
                        </FieldDescription>
                        {form.formState.errors.websocket?.max_idle_connections ? (
                          <FieldError>
                            {errorMessage(
                              form.formState.errors.websocket.max_idle_connections.message,
                            )}
                          </FieldError>
                        ) : null}
                      </Field>

                      <Field
                        data-invalid={Boolean(
                          form.formState.errors.websocket?.idle_timeout_seconds,
                        )}
                      >
                        <FieldLabel htmlFor="websocket_idle_timeout_seconds">
                          {t(settingsLabels["websocket.idle_timeout_seconds"])}
                        </FieldLabel>
                        <Input
                          id="websocket_idle_timeout_seconds"
                          type="number"
                          min={1}
                          max={3600}
                          aria-invalid={Boolean(
                            form.formState.errors.websocket?.idle_timeout_seconds,
                          )}
                          {...form.register("websocket.idle_timeout_seconds", {
                            valueAsNumber: true,
                          })}
                        />
                        {form.formState.errors.websocket?.idle_timeout_seconds ? (
                          <FieldError>
                            {errorMessage(
                              form.formState.errors.websocket.idle_timeout_seconds.message,
                            )}
                          </FieldError>
                        ) : null}
                      </Field>

                      <Field
                        data-invalid={Boolean(
                          form.formState.errors.websocket?.max_connection_age_seconds,
                        )}
                      >
                        <FieldLabel htmlFor="websocket_max_connection_age_seconds">
                          {t(settingsLabels["websocket.max_connection_age_seconds"])}
                        </FieldLabel>
                        <Input
                          id="websocket_max_connection_age_seconds"
                          type="number"
                          min={60}
                          max={3600}
                          aria-invalid={Boolean(
                            form.formState.errors.websocket?.max_connection_age_seconds,
                          )}
                          {...form.register("websocket.max_connection_age_seconds", {
                            valueAsNumber: true,
                          })}
                        />
                        {form.formState.errors.websocket?.max_connection_age_seconds ? (
                          <FieldError>
                            {errorMessage(
                              form.formState.errors.websocket.max_connection_age_seconds.message,
                            )}
                          </FieldError>
                        ) : null}
                      </Field>
                    </div>
                  </FieldGroup>
                </CardContent>
              </Card>
            </div> : null}

            {section === "affinity" ? <div>
              <SessionAffinityCard
                value={form.watch("session_affinity")}
                disabled={fieldsLocked}
                onChange={(session_affinity) =>
                  form.setValue("session_affinity", session_affinity, {
                    shouldDirty: true,
                    shouldValidate: true,
                  })
                }
                errors={{
                  maxEntries: errorMessage(
                    form.formState.errors.session_affinity?.max_entries?.message,
                  ),
                  defaultTtl: errorMessage(
                    form.formState.errors.session_affinity?.default_ttl_seconds?.message,
                  ),
                  rules: errorMessage(form.formState.errors.session_affinity?.rules?.message),
                }}
              />
            </div> : null}

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
