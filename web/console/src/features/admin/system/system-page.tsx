import { useEffect, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
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
import { PageHeader } from "@/components/shared/page-header";
import { DetailField } from "@/components/shared/detail-field";
import { StringListField } from "@/components/shared/string-list-field";
import { ApiError } from "@/api/errors";
import { useReload, useSystemSettings, useUpdateSystemSettings } from "@/features/admin/api";
import { SessionAffinityCard } from "@/features/admin/system/session-affinity-card";
import { useI18n } from "@/app/i18n";
import type { ScheduledTestingMode, SystemSettingsInput } from "@/api/types";

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
      stream_idle_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
    }),
    request_retry: z.object({
      enabled: z.boolean(),
      max_retries: z
        .number()
        .int()
        .min(1, "Maximum retries must be between 1 and 10.")
        .max(10, "Maximum retries must be between 1 and 10."),
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
    if (value.websocket.idle_timeout_seconds >= value.websocket.max_connection_age_seconds) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["websocket", "max_connection_age_seconds"],
        message: "Maximum WebSocket age must exceed the idle timeout.",
      });
    }
  });

type SystemSettingsValues = z.infer<typeof systemSettingsSchema>;

const defaultValues: SystemSettingsValues = {
  api_hosts: [],
  upstream: {
    connect_timeout_seconds: 10,
    response_header_timeout_seconds: 30,
    stream_idle_timeout_seconds: 90,
  },
  request_retry: {
    enabled: true,
    max_retries: 1,
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
        api_hosts: settings.data.data.api_hosts,
        upstream: settings.data.data.upstream,
        request_retry: settings.data.data.request_retry,
        passive_health: settings.data.data.passive_health,
        automatic_disable: {
          enabled: settings.data.data.automatic_disable.enabled,
          error_status_codes: settings.data.data.automatic_disable.error_status_codes.join(", "),
          error_message_keywords: settings.data.data.automatic_disable.error_message_keywords,
        },
        scheduled_testing: settings.data.data.scheduled_testing,
        session_affinity: settings.data.data.session_affinity,
        websocket: settings.data.data.websocket,
      });
    }
  }, [form, settings.data]);

  const errorMessage = (message: string | undefined) => (message ? t(message) : undefined);

  const save = async (values: SystemSettingsValues) => {
    if (!settings.data) return;
    try {
      const input: SystemSettingsInput = {
        api_hosts: values.api_hosts,
        upstream: values.upstream,
        request_retry: values.request_retry,
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
      const result = await updateSettings.mutateAsync({
        input,
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
          <form
            data-slot="system-settings-columns"
            onSubmit={form.handleSubmit(save)}
            className="grid items-start gap-6 xl:grid-cols-2"
          >
            <Card>
              <CardHeader>
                <CardTitle>{t("API hosts")}</CardTitle>
                <CardDescription>
                  {t(
                    "HTTP(S) base URLs shown on users' API Keys pages for copying into OpenAI-compatible clients.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <StringListField
                    id="api_hosts"
                    label={t("API hosts")}
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
                        {errorMessage(
                          form.formState.errors.upstream.response_header_timeout_seconds.message,
                        )}
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
                        {errorMessage(form.formState.errors.upstream.stream_idle_timeout_seconds.message)}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Request failover")}</CardTitle>
                <CardDescription>
                  {t(
                    "Before response headers arrive, connection failures, connect timeouts, and response-header timeouts can retry on distinct healthy channels. A timed-out upstream may still process the original request.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="request_retry_enabled">
                        {t("Enable automatic retry")}
                      </FieldLabel>
                      <FieldDescription>
                        {t(
                          "Retries never reuse a channel already attempted by the same client request.",
                        )}
                      </FieldDescription>
                    </FieldContent>
                    <Switch
                      id="request_retry_enabled"
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
                      {t("Maximum retries")}
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
                        {errorMessage(
                          form.formState.errors.passive_health.connection_failure_threshold.message,
                        )}
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
                        {errorMessage(form.formState.errors.passive_health.cooldown_seconds.message)}
                      </FieldError>
                    ) : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Automatic channel disable")}</CardTitle>
                <CardDescription>
                  {t(
                    "Matching upstream HTTP errors can temporarily remove opted-in channels from routing.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="automatic_disable_enabled">
                        {t("Enable automatic disable")}
                      </FieldLabel>
                      <FieldDescription>
                        {t(
                          "When disabled, channel-level automatic-disable permission has no effect.",
                        )}
                      </FieldDescription>
                    </FieldContent>
                    <Switch
                      id="automatic_disable_enabled"
                      checked={form.watch("automatic_disable.enabled")}
                      onCheckedChange={(checked) =>
                        form.setValue("automatic_disable.enabled", Boolean(checked), {
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
                      {t("HTTP error status codes")}
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
                    label={t("Error message keywords")}
                    value={form.watch("automatic_disable.error_message_keywords")}
                    onChange={(value) =>
                      form.setValue("automatic_disable.error_message_keywords", value, {
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

            <Card>
              <CardHeader>
                <CardTitle>{t("Scheduled channel tests")}</CardTitle>
                <CardDescription>
                  {t(
                    "Direct non-streaming test requests use each channel's selected test model. Their token usage and costs are logged and billed to a system-owned administrator API key.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field>
                    <FieldLabel>{t("Test mode")}</FieldLabel>
                    <Select
                      value={form.watch("scheduled_testing.mode")}
                      onValueChange={(value) =>
                        form.setValue(
                          "scheduled_testing.mode",
                          value as ScheduledTestingMode,
                          { shouldValidate: true },
                        )
                      }
                    >
                      <SelectTrigger>
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
                      {t("Test interval (minutes)")}
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
                    <FieldLabel htmlFor="scheduled_testing_prompt">{t("Test prompt")}</FieldLabel>
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
                        {t("Automatically recover")}
                      </FieldLabel>
                      <FieldDescription>
                        {t(
                          "Restore a temporarily disabled channel after its scheduled test succeeds.",
                        )}
                      </FieldDescription>
                    </FieldContent>
                    <Switch
                      id="scheduled_testing_auto_recover"
                      checked={form.watch("scheduled_testing.auto_recover")}
                      onCheckedChange={(checked) =>
                        form.setValue("scheduled_testing.auto_recover", Boolean(checked), {
                          shouldValidate: true,
                        })
                      }
                    />
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <div className="xl:col-span-2">
              <Card>
                <CardHeader>
                  <CardTitle>{t("Responses WebSocket")}</CardTitle>
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
                          {t("Enable Responses WebSocket")}
                        </FieldLabel>
                        <FieldDescription>
                          {t(
                            "Disabled systems reject new WebSocket upgrades and discard idle upstream connections.",
                          )}
                        </FieldDescription>
                      </FieldContent>
                      <Switch
                        id="websocket_enabled"
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
                          {t("Maximum idle connections")}
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
                          {t("Idle timeout (seconds)")}
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
                          {t("Maximum connection age (seconds)")}
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
                        <FieldDescription>
                          {t("Must be greater than the idle timeout.")}
                        </FieldDescription>
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
            </div>

            <div className="xl:col-span-2">
              <SessionAffinityCard
                value={form.watch("session_affinity")}
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
            </div>

            <Button
              type="submit"
              className="w-fit xl:col-span-2"
              disabled={updateSettings.isPending}
            >
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
