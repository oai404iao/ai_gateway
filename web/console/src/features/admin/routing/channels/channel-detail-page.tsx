import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { RefreshCwIcon } from "lucide-react";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
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
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { TransformDocumentEditor } from "@/features/admin/transforms/transform-document-editor";
import { DetailField } from "@/components/shared/detail-field";
import { ApiKeyValue } from "@/components/shared/api-key-value";
import { StringListField } from "@/components/shared/string-list-field";
import { DecimalField, NullableNumberField } from "@/components/shared/decimal-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { ChannelModelPickerDialog } from "@/features/admin/routing/channels/channel-model-picker-dialog";
import {
  useChannel,
  useChannelGroups,
  useChannels,
  useConfigTemplates,
  useCreateChannel,
  useDiscoverChannelModels,
  useModelRules,
  useProxies,
  useUpdateChannel,
} from "@/features/admin/api";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type {
  ApiFormat,
  ChannelCreateInput,
  ChannelInput,
  ChannelModelDiscoveryInput,
  UpstreamAuthKind,
} from "@/api/types";
import { UPSTREAM_AUTH_KINDS, apiFormatLabel, upstreamAuthKindLabel } from "@/lib/permissions";
import { channelUpdateInvalidatesRouting } from "@/features/admin/routing/routing-validation";
import { useI18n } from "@/app/i18n";
import { formatDecimal } from "@/lib/formatters";

function isAllowedBaseUrl(value: string): boolean {
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
}

function isNonNegativeDecimal(value: string): boolean {
  return (
    /^(?:0|[1-9]\d*)(?:\.\d+)?$/.test(value) &&
    Number.isFinite(Number(value))
  );
}

const schema = z.object({
  channel_group_id: z.string().min(1, "Pick a channel group."),
  api_format: z.enum(["open_ai_chat_completions", "open_ai_responses"]),
  name: z.string().trim().min(1, "Name is required.").max(100),
  base_url: z
    .string()
    .trim()
    .refine(
      isAllowedBaseUrl,
      "Enter an HTTP(S) URL without credentials, query parameters, or a fragment.",
  ),
  enabled: z.boolean(),
  supports_websocket: z.boolean(),
  status_statistics_enabled: z.boolean(),
  auto_disable_allowed: z.boolean(),
  weight: z.number().int().min(1, "Weight must be at least 1."),
  billing_multiplier: z
    .string()
    .trim()
    .refine(
      isNonNegativeDecimal,
      "Billing multiplier must be zero or greater.",
    ),
  proxy_id: z.string().nullable(),
  config_template_id: z.string().nullable(),
  override_document: z.string(),
  connect_timeout_ms: z.number().int().positive().nullable(),
  response_header_timeout_ms: z.number().int().positive().nullable(),
  stream_idle_timeout_ms: z.number().int().positive().nullable(),
  upstream_auth_kind: z.enum(["none", "bearer", "header"]),
  upstream_auth_header_name: z.string().trim().nullable(),
  upstream_api_key: z.string(),
  available_models: z.array(z.string().trim().min(1, "Model ID is required.")),
  test_model: z.string().nullable(),
}).superRefine((value, context) => {
  if (value.upstream_auth_kind === "header" && !value.upstream_auth_header_name) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["upstream_auth_header_name"],
      message: "A custom header name is required.",
    });
  }
  if (new Set(value.available_models).size !== value.available_models.length) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["available_models"],
      message: "Available model IDs must be unique.",
    });
  }
  if (value.test_model && !value.available_models.includes(value.test_model)) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["test_model"],
      message: "Choose a test model from the available upstream models.",
    });
  }
  if (value.supports_websocket && value.api_format !== "open_ai_responses") {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["supports_websocket"],
      message: "Only Responses channels can support WebSocket forwarding.",
    });
  }
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  channel_group_id: "",
  api_format: "open_ai_chat_completions",
  name: "",
  base_url: "",
  enabled: true,
  supports_websocket: false,
  status_statistics_enabled: false,
  auto_disable_allowed: false,
  weight: 100,
  billing_multiplier: "1",
  proxy_id: null,
  config_template_id: null,
  override_document: "{}",
  connect_timeout_ms: null,
  response_header_timeout_ms: null,
  stream_idle_timeout_ms: null,
  upstream_auth_kind: "bearer",
  upstream_auth_header_name: null,
  upstream_api_key: "",
  available_models: [],
  test_model: null,
};

export function ChannelDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useChannel(id);
  const create = useCreateChannel();
  const update = useUpdateChannel(id);
  const discoverModels = useDiscoverChannelModels();
  const groups = useChannelGroups();
  const channels = useChannels();
  const rules = useModelRules();
  const proxies = useProxies();
  const templates = useConfigTemplates();
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const modelPickerTriggerId = "channel-model-picker-trigger";
  const [discoveredModels, setDiscoveredModels] = useState<string[]>([]);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [overrideDocumentValidation, setOverrideDocumentValidation] = useState<string | null>(
    null,
  );

  useEffect(() => {
    if (data) {
      if (data.data.provider_managed) {
        navigate(`/admin/providers/codex-oauth/${data.data.channel_group_id}`, {
          replace: true,
        });
        return;
      }
      setState({
        channel_group_id: data.data.channel_group_id,
        api_format: data.data.api_format,
        name: data.data.name,
        base_url: data.data.base_url,
        enabled: data.data.enabled,
        supports_websocket: data.data.supports_websocket,
        status_statistics_enabled: data.data.status_statistics_enabled,
        auto_disable_allowed: data.data.auto_disable_allowed,
        weight: data.data.weight,
        billing_multiplier: data.data.billing_multiplier,
        proxy_id: data.data.proxy_id,
        config_template_id: data.data.config_template_id,
        override_document: JSON.stringify(data.data.override_document, null, 2) ?? "{}",
        connect_timeout_ms: data.data.connect_timeout_ms,
        response_header_timeout_ms: data.data.response_header_timeout_ms,
        stream_idle_timeout_ms: data.data.stream_idle_timeout_ms,
        upstream_auth_kind: data.data.upstream_auth_kind,
        upstream_auth_header_name: data.data.upstream_auth_header_name,
        upstream_api_key: data.data.upstream_api_key ?? "",
        available_models: data.data.available_models,
        test_model: data.data.test_model,
      });
      setOverrideDocumentValidation(null);
    }
  }, [data, navigate]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  // When the group changes, align the channel format to the group's format.
  const selectedGroup = useMemo(
    () => groups.data?.find((group) => group.id === state.channel_group_id),
    [groups.data, state.channel_group_id],
  );
  const configTemplateOptions = useMemo(
    () =>
      (templates.data ?? [])
        .map((template) => {
          const formatCompatible =
            template.api_format === null || template.api_format === state.api_format;
          return {
            ...template,
            formatCompatible,
            selectable: template.enabled && formatCompatible,
          };
        })
        .sort(
          (left, right) =>
            Number(right.selectable) - Number(left.selectable) ||
            left.name.localeCompare(right.name),
        ),
    [state.api_format, templates.data],
  );

  const discoverUpstreamModels = async () => {
    if (overrideDocumentValidation) {
      toast.error(t(overrideDocumentValidation));
      return;
    }
    if (!isAllowedBaseUrl(state.base_url.trim())) {
      toast.error(
        t("Enter an HTTP(S) URL without credentials, query parameters, or a fragment."),
      );
      return;
    }
    if (
      state.upstream_auth_kind === "header" &&
      !state.upstream_auth_header_name?.trim()
    ) {
      toast.error(t("A custom header name is required."));
      return;
    }

    let overrideDocument: unknown;
    try {
      overrideDocument = state.override_document.trim()
        ? JSON.parse(state.override_document)
        : data?.data.override_document ?? {};
    } catch {
      toast.error(t("Override document is not valid JSON."));
      return;
    }
    if (
      typeof overrideDocument !== "object" ||
      overrideDocument === null ||
      Array.isArray(overrideDocument)
    ) {
      toast.error(t("Override document must be a JSON object."));
      return;
    }

    const upstreamApiKey =
      state.upstream_auth_kind === "none"
        ? null
        : state.upstream_api_key.trim()
          ? state.upstream_api_key
          : data?.data.upstream_api_key ?? null;
    if (state.upstream_auth_kind !== "none" && !upstreamApiKey) {
      toast.error(t("An upstream API key is required when upstream auth is enabled."));
      return;
    }

    const input: ChannelModelDiscoveryInput = {
      api_format: state.api_format,
      base_url: state.base_url.trim(),
      proxy_id: state.proxy_id,
      config_template_id: state.config_template_id,
      override_document: overrideDocument,
      connect_timeout_ms: state.connect_timeout_ms,
      response_header_timeout_ms: state.response_header_timeout_ms,
      stream_idle_timeout_ms: state.stream_idle_timeout_ms,
      upstream_auth_kind: state.upstream_auth_kind,
      upstream_auth_header_name:
        state.upstream_auth_kind === "header"
          ? state.upstream_auth_header_name?.trim() || null
          : null,
      upstream_api_key: upstreamApiKey,
    };

    try {
      const response = await discoverModels.mutateAsync(input);
      setDiscoveredModels(response.models);
      setModelPickerOpen(true);
    } catch (error) {
      if (
        error instanceof ApiError &&
        error.code === "channel_models_invalid_configuration"
      ) {
        toast.error(
          t(
            "The channel settings are not valid for model discovery. Check the base URL, timeouts, proxy, template, transforms, and authentication.",
          ),
        );
      } else if (
        error instanceof ApiError &&
        error.code === "upstream_models_timeout"
      ) {
        toast.error(t("Timed out while fetching upstream models."));
      } else {
        toast.error(
          t(
            "Could not fetch upstream models. Verify the base URL, credential, proxy, and transforms.",
          ),
        );
      }
    }
  };

  const submit = async () => {
    if (overrideDocumentValidation) {
      toast.error(t(overrideDocumentValidation));
      return;
    }
    let overrideDocument: unknown;
    try {
      if (state.override_document.trim()) {
        overrideDocument = JSON.parse(state.override_document);
      } else if (isNew) {
        overrideDocument = {};
      }
    } catch {
      toast.error(t("Override document is not valid JSON."));
      return;
    }
    if (
      overrideDocument !== undefined &&
      (typeof overrideDocument !== "object" ||
        overrideDocument === null ||
        Array.isArray(overrideDocument))
    ) {
      toast.error(t("Override document must be a JSON object."));
      return;
    }
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    if (parsed.data.enabled && !selectedGroup?.enabled) {
      toast.error(t("Choose an enabled channel group before enabling this channel."));
      return;
    }
    if (
      !isNew &&
      data &&
      channels.data &&
      groups.data &&
      rules.data &&
      channelUpdateInvalidatesRouting(
        data.data.id,
        parsed.data,
        channels.data,
        groups.data,
        rules.data,
      )
    ) {
      toast.error(
        t(
          "Save blocked: this change would make the routing configuration invalid. Keep an eligible channel or update dependent rules first.",
        ),
      );
      return;
    }
    if (
      isNew &&
      parsed.data.upstream_auth_kind !== "none" &&
      parsed.data.upstream_api_key.trim() === ""
    ) {
      toast.error(t("An upstream API key is required when upstream auth is enabled."));
      return;
    }
    setValidation(null);
    setSubmitting(true);
    try {
      if (isNew) {
        const input: ChannelCreateInput = {
          channel_group_id: parsed.data.channel_group_id,
          api_format: parsed.data.api_format as ApiFormat,
          name: parsed.data.name,
          base_url: parsed.data.base_url,
          enabled: parsed.data.enabled,
          supports_websocket: parsed.data.supports_websocket,
          status_statistics_enabled: parsed.data.status_statistics_enabled,
          auto_disable_allowed: parsed.data.auto_disable_allowed,
          weight: parsed.data.weight,
          billing_multiplier: parsed.data.billing_multiplier,
          proxy_id: parsed.data.proxy_id,
          config_template_id: parsed.data.config_template_id,
          override_document: overrideDocument ?? {},
          connect_timeout_ms: parsed.data.connect_timeout_ms,
          response_header_timeout_ms: parsed.data.response_header_timeout_ms,
          stream_idle_timeout_ms: parsed.data.stream_idle_timeout_ms,
          upstream_auth_kind: parsed.data.upstream_auth_kind as UpstreamAuthKind,
          upstream_auth_header_name:
            parsed.data.upstream_auth_kind === "header"
              ? parsed.data.upstream_auth_header_name
              : null,
          upstream_api_key:
            parsed.data.upstream_auth_kind === "none"
              ? null
              : parsed.data.upstream_api_key || null,
          available_models: parsed.data.available_models,
          test_model: parsed.data.test_model,
        };
        await create.mutateAsync(input);
        toast.success(t("Channel created"));
        navigate("/admin/routing/channels", { replace: true });
      } else {
        // On edit, omit upstream_api_key when blank to keep the current secret.
        const input: ChannelInput = {
          channel_group_id: parsed.data.channel_group_id,
          api_format: parsed.data.api_format as ApiFormat,
          name: parsed.data.name,
          base_url: parsed.data.base_url,
          enabled: parsed.data.enabled,
          supports_websocket: parsed.data.supports_websocket,
          status_statistics_enabled: parsed.data.status_statistics_enabled,
          auto_disable_allowed: parsed.data.auto_disable_allowed,
          weight: parsed.data.weight,
          billing_multiplier: parsed.data.billing_multiplier,
          proxy_id: parsed.data.proxy_id,
          config_template_id: parsed.data.config_template_id,
          connect_timeout_ms: parsed.data.connect_timeout_ms,
          response_header_timeout_ms: parsed.data.response_header_timeout_ms,
          stream_idle_timeout_ms: parsed.data.stream_idle_timeout_ms,
          upstream_auth_kind: parsed.data.upstream_auth_kind as UpstreamAuthKind,
          upstream_auth_header_name:
            parsed.data.upstream_auth_kind === "header"
              ? parsed.data.upstream_auth_header_name
              : null,
          available_models: parsed.data.available_models,
          test_model: parsed.data.test_model,
        };
        if (overrideDocument !== undefined) {
          input.override_document = overrideDocument;
        }
        if (parsed.data.upstream_auth_kind === "none") {
          input.upstream_api_key = null;
        } else if (parsed.data.upstream_api_key !== "") {
          input.upstream_api_key = parsed.data.upstream_api_key;
        }
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Channel updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This channel was changed elsewhere. Reloading."));
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)?.message;
    return message ? t(message) : undefined;
  };

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New channel") : state.name || t("Channel")}
        description={t("An upstream endpoint with weight, timeouts, and credential injection.")}
        backPath="/admin/routing/channels"
        backLabel={t("Back to channels")}
        isLoading={isLoading}
        error={error}
        hasData={isNew || Boolean(data)}
        detailCard={
          !isNew && data ? (
            <Card>
            <CardHeader>
              <CardTitle>{data.data.name}</CardTitle>
              <CardDescription className="font-mono">{data.data.base_url}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField
                  label={t("Enabled")}
                  value={<StatusBadge value={data.data.enabled} />}
                />
                <DetailField
                  label={t("Responses WebSocket")}
                  value={<StatusBadge value={data.data.supports_websocket} />}
                />
                <DetailField
                  label={t("Auto-disabled")}
                  value={<StatusBadge value={data.data.auto_disabled} />}
                />
                <DetailField
                  label={t("Auto-disable reason")}
                  value={data.data.auto_disabled_reason ?? "—"}
                />
                <DetailField
                  label={t("Allow automatic disable")}
                  value={<StatusBadge value={data.data.auto_disable_allowed} />}
                />
                <DetailField
                  label={t("Scheduled test model")}
                  value={data.data.test_model ?? "—"}
                  mono={Boolean(data.data.test_model)}
                />
                <DetailField
                  label={t("Status statistics")}
                  value={<StatusBadge value={data.data.status_statistics_enabled} />}
                />
                <DetailField
                  label={t("Credential configured")}
                  value={data.data.upstream_credential_configured ? t("yes") : t("no")}
                />
                <DetailField label={t("Weight")} value={data.data.weight} />
                <DetailField
                  label={t("Billing multiplier")}
                  value={formatDecimal(data.data.billing_multiplier)}
                />
                <DetailField
                  label={t("Upstream API key")}
                  value={
                    data.data.upstream_api_key ? (
                      <ApiKeyValue value={data.data.upstream_api_key} className="max-w-xl" />
                    ) : (
                      "—"
                    )
                  }
                  className="sm:col-span-2"
                />
              </dl>
            </CardContent>
            </Card>
          ) : null
        }
        editCard={
          <div
            data-slot="channel-edit-layout"
            className="grid items-start gap-6 xl:grid-cols-2"
          >
            <Card>
              <CardHeader>
                <CardTitle>{t("Routing and identity")}</CardTitle>
                <CardDescription>
                  {t("The channel format must match its group's format.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field data-invalid={Boolean(fieldError("channel_group_id"))}>
                    <FieldLabel>{t("Channel group")}</FieldLabel>
                    <Select
                      value={state.channel_group_id || "__none__"}
                      onValueChange={(value) => {
                        const group = groups.data?.find((item) => item.id === value);
                        patch({
                          channel_group_id: value === "__none__" ? "" : value,
                          api_format: (group?.api_format ?? state.api_format) as ApiFormat,
                          supports_websocket:
                            group?.api_format === "open_ai_responses"
                              ? state.supports_websocket
                              : false,
                        });
                      }}
                    >
                      <SelectTrigger aria-invalid={Boolean(fieldError("channel_group_id"))}>
                        <SelectValue placeholder={t("Pick a group")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value="__none__">{t("None")}</SelectItem>
                          {groups.data
                            ?.filter((group) => group.connector_kind === "openai_compatible")
                            .map((group) => (
                            <SelectItem
                              key={group.id}
                              value={group.id}
                              disabled={state.enabled && !group.enabled}
                            >
                              {group.name} ({apiFormatLabel(group.api_format)})
                            </SelectItem>
                            ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    {fieldError("channel_group_id") ? (
                      <FieldError>{fieldError("channel_group_id")}</FieldError>
                    ) : null}
                  </Field>
                  <Field>
                    <FieldLabel>{t("API format")}</FieldLabel>
                    <Input
                      value={apiFormatLabel(selectedGroup?.api_format ?? state.api_format)}
                      disabled
                    />
                  </Field>
                  <Field data-invalid={Boolean(fieldError("name"))}>
                    <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                    <Input
                      id="name"
                      value={state.name}
                      onChange={(event) => patch({ name: event.target.value })}
                      aria-invalid={Boolean(fieldError("name"))}
                    />
                    {fieldError("name") ? <FieldError>{fieldError("name")}</FieldError> : null}
                  </Field>
                  <Field data-invalid={Boolean(fieldError("base_url"))}>
                    <FieldLabel htmlFor="base_url">{t("Base URL")}</FieldLabel>
                    <Input
                      id="base_url"
                      value={state.base_url}
                      onChange={(event) => patch({ base_url: event.target.value })}
                      placeholder="https://api.upstream.com"
                      aria-invalid={Boolean(fieldError("base_url"))}
                    />
                    {fieldError("base_url") ? <FieldError>{fieldError("base_url")}</FieldError> : null}
                  </Field>
                  <Field data-invalid={Boolean(fieldError("weight"))}>
                    <FieldLabel htmlFor="weight">{t("Weight")}</FieldLabel>
                    <Input
                      id="weight"
                      type="number"
                      min={1}
                      value={state.weight}
                      onChange={(event) =>
                        patch({ weight: Math.max(1, Number(event.target.value) || 1) })
                      }
                      aria-invalid={Boolean(fieldError("weight"))}
                    />
                    {fieldError("weight") ? <FieldError>{fieldError("weight")}</FieldError> : null}
                  </Field>
                  <DecimalField
                    id="billing_multiplier"
                    label={t("Billing multiplier")}
                    value={state.billing_multiplier}
                    onChange={(value) => patch({ billing_multiplier: value })}
                    error={fieldError("billing_multiplier")}
                    description={t(
                      "Multiplies the upstream model price used for request settlement.",
                    )}
                    required
                  />
                </FieldGroup>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Upstream connection")}</CardTitle>
                <CardDescription>
                  {t("Proxy, template, authentication, and credential overrides.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <Field>
                    <FieldLabel>{t("Proxy")}</FieldLabel>
                    <Select
                      value={state.proxy_id ?? "__none__"}
                      onValueChange={(value) => patch({ proxy_id: value === "__none__" ? null : value })}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value="__none__">{t("None")}</SelectItem>
                          {proxies.data?.filter((proxy) => proxy.enabled).map((proxy) => (
                            <SelectItem key={proxy.id} value={proxy.id}>
                              {proxy.name}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </Field>
                  <Field>
                    <FieldLabel htmlFor="config_template_id">
                      {t("Config template")}
                    </FieldLabel>
                    <Select
                      value={state.config_template_id ?? "__none__"}
                      onValueChange={(value) =>
                        patch({ config_template_id: value === "__none__" ? null : value })
                      }
                    >
                      <SelectTrigger id="config_template_id">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value="__none__">{t("None")}</SelectItem>
                          {configTemplateOptions.map((template) => (
                            <SelectItem
                              key={template.id}
                              value={template.id}
                              disabled={!template.selectable}
                            >
                              {template.name} · {template.api_format === null
                                ? t("All formats")
                                : apiFormatLabel(template.api_format)}
                              {!template.enabled ? ` · ${t("Disabled")}` : ""}
                              {!template.formatCompatible
                                ? ` · ${t("Incompatible format")}`
                                : ""}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldDescription>
                      {t(
                        "Only enabled templates matching this channel's API format can be selected.",
                      )}
                    </FieldDescription>
                  </Field>
                  <Field>
                    <FieldLabel>{t("Upstream auth kind")}</FieldLabel>
                    <Select
                      value={state.upstream_auth_kind}
                      onValueChange={(value) => {
                        const upstreamAuthKind = value as UpstreamAuthKind;
                        patch({
                          upstream_auth_kind: upstreamAuthKind,
                          upstream_auth_header_name:
                            upstreamAuthKind === "header"
                              ? state.upstream_auth_header_name
                              : null,
                        });
                      }}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          {UPSTREAM_AUTH_KINDS.map((kind) => (
                            <SelectItem key={kind} value={kind}>
                              {upstreamAuthKindLabel(kind)}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </Field>
                  {state.upstream_auth_kind === "header" ? (
                    <Field data-invalid={Boolean(fieldError("upstream_auth_header_name"))}>
                      <FieldLabel htmlFor="header_name">{t("Header name")}</FieldLabel>
                      <Input
                        id="header_name"
                        value={state.upstream_auth_header_name ?? ""}
                        onChange={(event) =>
                          patch({ upstream_auth_header_name: event.target.value || null })
                        }
                        placeholder="x-api-key"
                        aria-invalid={Boolean(fieldError("upstream_auth_header_name"))}
                      />
                      {fieldError("upstream_auth_header_name") ? (
                        <FieldError>{fieldError("upstream_auth_header_name")}</FieldError>
                      ) : null}
                    </Field>
                  ) : null}
                  {state.upstream_auth_kind !== "none" ? (
                    <Field>
                      <FieldLabel htmlFor="upstream_api_key">
                        {t("Upstream API key")}{" "}
                        {!isNew ? (
                          <span className="text-xs text-muted-foreground">
                            {t("(leave blank to keep current)")}
                          </span>
                        ) : null}
                      </FieldLabel>
                      <Input
                        id="upstream_api_key"
                        value={state.upstream_api_key}
                        onChange={(event) => patch({ upstream_api_key: event.target.value })}
                        autoComplete="off"
                      />
                    </Field>
                  ) : null}
                  <NullableNumberField
                    label={t("Connect timeout (ms)")}
                    value={state.connect_timeout_ms}
                    onChange={(value) => patch({ connect_timeout_ms: value })}
                  />
                  <NullableNumberField
                    label={t("Response header timeout (ms)")}
                    value={state.response_header_timeout_ms}
                    onChange={(value) => patch({ response_header_timeout_ms: value })}
                  />
                  <NullableNumberField
                    label={t("Stream idle timeout (ms)")}
                    value={state.stream_idle_timeout_ms}
                    onChange={(value) => patch({ stream_idle_timeout_ms: value })}
                  />
                </FieldGroup>
              </CardContent>
            </Card>

            <Card className="xl:col-span-2">
              <CardHeader>
                <CardTitle>{t("Models and timeouts")}</CardTitle>
                <CardDescription>
                  {t("Available upstream models, scheduled checks, and timeout overrides.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup className="grid gap-5 xl:grid-cols-2">
                  <StringListField
                    id="available_models"
                    className="xl:col-span-2"
                    variant="tokens"
                    label={t("Available upstream models")}
                    value={state.available_models}
                    onChange={(value) =>
                      patch({
                        available_models: value,
                        test_model: state.test_model && !value.includes(state.test_model)
                          ? null
                          : state.test_model,
                      })
                    }
                    placeholder={t("Enter an upstream model ID")}
                    description={t("Press Enter or Add to include a model.")}
                    error={fieldError("available_models")}
                    action={
                      <Button
                        id={modelPickerTriggerId}
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={discoverUpstreamModels}
                        disabled={discoverModels.isPending}
                      >
                        {discoverModels.isPending ? (
                          <Spinner data-icon="inline-start" />
                        ) : (
                          <RefreshCwIcon data-icon="inline-start" />
                        )}
                        {t("Fetch models")}
                      </Button>
                    }
                  />
                  <Field data-invalid={Boolean(fieldError("test_model"))}>
                    <FieldLabel>{t("Scheduled test model")}</FieldLabel>
                    <Select
                      value={state.test_model ?? "__none__"}
                      onValueChange={(value) =>
                        patch({ test_model: value === "__none__" ? null : value })
                      }
                    >
                      <SelectTrigger aria-invalid={Boolean(fieldError("test_model"))}>
                        <SelectValue placeholder={t("Select a test model")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value="__none__">{t("None")}</SelectItem>
                          {state.available_models.map((model) => (
                            <SelectItem key={model} value={model}>
                              {model}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldDescription>
                      {t(
                        "Periodic scheduled tests use this model. It must be one of the available upstream models and have a configured price.",
                      )}
                    </FieldDescription>
                    {fieldError("test_model") ? <FieldError>{fieldError("test_model")}</FieldError> : null}
                  </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <Card className="xl:col-span-2">
              <CardHeader>
                <CardTitle>{t("Transform override")}</CardTitle>
                <CardDescription>
                  {t("Optional constrained transform document.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <Field>
                  <FieldLabel>
                    {t("Override document (JSON)")}
                  </FieldLabel>
                  <FieldDescription>
                    {t("Optional constrained transform document.")}
                  </FieldDescription>
                  <TransformDocumentEditor
                    value={state.override_document}
                    onChange={(override_document) => patch({ override_document })}
                    fixedApiFormat={state.api_format}
                    onVisualValidationChange={setOverrideDocumentValidation}
                  />
                </Field>
              </CardContent>
            </Card>

            <Card className="xl:col-span-2">
              <CardHeader>
                <CardTitle>{t("Availability and automation")}</CardTitle>
                <CardDescription>
                  {t("Routing state, status reporting, and automatic disable behavior.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup className="grid gap-5 xl:grid-cols-2">
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="status_statistics_enabled">
                        {t("Status statistics")}
                      </FieldLabel>
                      <FieldDescription>
                        {t("Include this channel in the channel status report.")}
                      </FieldDescription>
                    </FieldContent>
                    <Switch
                      id="status_statistics_enabled"
                      checked={state.status_statistics_enabled}
                      onCheckedChange={(checked) =>
                        patch({ status_statistics_enabled: Boolean(checked) })
                      }
                    />
                  </Field>
                  <Field
                    orientation="horizontal"
                    data-invalid={Boolean(fieldError("supports_websocket"))}
                  >
                    <FieldContent>
                      <FieldLabel htmlFor="supports_websocket">
                        {t("Supports Responses WebSocket")}
                      </FieldLabel>
                      <FieldDescription>
                        {state.api_format === "open_ai_responses"
                          ? t(
                              "Allow this channel to receive WebSocket requests when the system and user are also enabled.",
                            )
                          : t("Only OpenAI Responses channels can enable WebSocket forwarding.")}
                      </FieldDescription>
                      {fieldError("supports_websocket") ? (
                        <FieldError>{fieldError("supports_websocket")}</FieldError>
                      ) : null}
                    </FieldContent>
                    <Switch
                      id="supports_websocket"
                      checked={state.supports_websocket}
                      disabled={state.api_format !== "open_ai_responses"}
                      onCheckedChange={(checked) =>
                        patch({ supports_websocket: Boolean(checked) })
                      }
                    />
                  </Field>
                  <Field orientation="horizontal">
                    <FieldContent>
                      <FieldLabel htmlFor="auto_disable_allowed">
                        {t("Allow automatic disable")}
                      </FieldLabel>
                      <FieldDescription>
                        {t(
                          "Allow matching system automatic-disable rules to temporarily remove this channel from routing.",
                        )}
                      </FieldDescription>
                    </FieldContent>
                    <Switch
                      id="auto_disable_allowed"
                      checked={state.auto_disable_allowed}
                      onCheckedChange={(checked) =>
                        patch({ auto_disable_allowed: Boolean(checked) })
                      }
                    />
                  </Field>
                  <Field orientation="horizontal">
                    <FieldLabel htmlFor="channel_enabled">{t("Enabled")}</FieldLabel>
                    <Switch
                      id="channel_enabled"
                      checked={state.enabled}
                      onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
                </FieldGroup>
              </CardContent>
            </Card>

            <Button
              className="w-fit xl:col-span-2"
              onClick={submit}
              disabled={submitting}
            >
              {submitting ? <Spinner data-icon="inline-start" /> : null}
              {isNew ? t("Create channel") : t("Save channel")}
            </Button>
          </div>
        }
      />
      <ChannelModelPickerDialog
        open={modelPickerOpen}
        onOpenChange={setModelPickerOpen}
        triggerId={modelPickerTriggerId}
        models={discoveredModels}
        currentModels={state.available_models}
        onApply={(available_models) =>
          patch({
            available_models,
            test_model:
              state.test_model && !available_models.includes(state.test_model)
                ? null
                : state.test_model,
          })
        }
      />
    </>
  );
}
