import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { DecimalField } from "@/components/shared/decimal-field";
import { StringListField } from "@/components/shared/string-list-field";
import { ChannelModelPickerDialog } from "@/features/admin/routing/channels/channel-model-picker-dialog";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useChannelCapability,
  useConfigTemplates,
  useCreateChannelCapability,
  useDeleteChannelCapability,
  useDiscoverChannelModels,
  useLogicalChannels,
  useRecoverCapability,
  useUpdateChannelCapability,
  useUpstreamAccesses,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import {
  API_OPERATIONS,
  CAPABILITY_TRANSPORTS,
  REQUEST_COMPRESSIONS,
  apiOperationLabel,
  capabilityTransportLabel,
} from "@/lib/permissions";
import type {
  ApiOperation,
  CapabilitySettings,
  CapabilityTransport,
  ChannelCapabilityInput,
  RequestCompression,
} from "@/api/types";

const NONE = "__none__";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function isJson(value: string): boolean {
  try {
    JSON.parse(value);
    return true;
  } catch {
    return false;
  }
}

const schema = z.object({
  channel_id: z.string().regex(UUID, "invalid"),
  operation: z.enum([
    "chat_completions",
    "responses",
    "standalone_web_search",
    "images_generation",
    "images_edit",
  ]),
  transports: z
    .array(z.enum(["http_json", "http_sse", "websocket", "multipart"]))
    .min(1),
  enabled: z.boolean(),
  available_models: z.array(z.string()),
  request_compression: z.enum(["default", "zstd"]),
  test_model: z.string(),
  test_pricing_model_id: z
    .string()
    .refine((value) => value === "" || UUID.test(value), "invalid"),
  auto_disable_allowed: z.boolean(),
  status_statistics_enabled: z.boolean(),
  config_template_id: z
    .string()
    .refine((value) => value === "" || UUID.test(value), "invalid"),
  override_document: z.string().refine(isJson, "invalid JSON"),
  billing_multiplier: z.string().regex(/^\d+(?:\.\d+)?$/),
});
type FormValues = z.infer<typeof schema>;

const defaults: FormValues = {
  channel_id: "",
  operation: "responses",
  transports: ["http_json"],
  enabled: true,
  available_models: [],
  request_compression: "default",
  test_model: "",
  test_pricing_model_id: "",
  auto_disable_allowed: false,
  status_statistics_enabled: false,
  config_template_id: "",
  override_document: "{}",
  billing_multiplier: "1",
};

export function CapabilityDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { t } = useI18n();
  const query = useChannelCapability(id);
  const channels = useLogicalChannels();
  const accesses = useUpstreamAccesses();
  const discover = useDiscoverChannelModels();
  const [pickingModels, setPickingModels] = useState(false);
  const templates = useConfigTemplates();
  const create = useCreateChannelCapability();
  const update = useUpdateChannelCapability(id);
  const remove = useDeleteChannelCapability(id);
  const recover = useRecoverCapability(id);
  const [confirmingRecovery, setConfirmingRecovery] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const form = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { ...defaults, channel_id: params.get("channel") ?? defaults.channel_id },
  });
  const capability = query.data?.data;
  const selectedChannel = channels.data?.find((channel) => channel.id === form.watch("channel_id"));
  const selectedAccess = accesses.data?.find((access) => access.id === selectedChannel?.access_id);
  const busy = create.isPending || update.isPending || remove.isPending || recover.isPending;
  const channelNames = useMemo(
    () => new Map((channels.data ?? []).map((channel) => [channel.id, channel.name])),
    [channels.data],
  );

  const discoverModels = async () => {
    if (!selectedChannel || !selectedAccess) return;
    const values = form.getValues();
    if (!isJson(values.override_document)) {
      form.setError("override_document", { message: "invalid JSON" });
      return;
    }
    try {
      await discover.mutateAsync({
        api_format: values.operation === "chat_completions" ? "open_ai_chat_completions"
          : values.operation.startsWith("images_") ? "open_ai_images" : "open_ai_responses",
        base_url: selectedAccess.base_url,
        credential_id: selectedChannel.credential_id,
        proxy_id: selectedAccess.proxy_id,
        connect_timeout_ms: selectedAccess.connect_timeout_ms,
        response_header_timeout_ms: selectedAccess.response_header_timeout_ms,
        stream_idle_timeout_ms: selectedAccess.stream_idle_timeout_ms,
        config_template_id: values.config_template_id || null,
        override_document: JSON.parse(values.override_document),
      });
      setPickingModels(true);
    } catch (error) {
      toast.error(t(controlPlaneMutationErrorMessage(error)));
    }
  };

  useEffect(() => {
    if (capability) {
      const settings = capability.settings;
      form.reset({
        channel_id: capability.channel_id,
        operation: settings.operation,
        transports: settings.transports,
        enabled: settings.enabled,
        available_models: settings.available_models,
        request_compression: settings.request_compression,
        test_model: settings.test_model ?? "",
        test_pricing_model_id: settings.test_pricing_model_id ?? "",
        auto_disable_allowed: settings.auto_disable_allowed,
        status_statistics_enabled: capability.status_statistics_enabled,
        config_template_id: capability.config_template_id ?? "",
        override_document: JSON.stringify(capability.override_document ?? {}, null, 2),
        billing_multiplier: capability.billing_multiplier,
      });
    }
  }, [capability, form]);

  const submit = form.handleSubmit(async (values) => {
    const settings: CapabilitySettings = {
      operation: values.operation,
      transports: values.transports,
      enabled: values.enabled,
      available_models: values.available_models.map((model) => model.trim()).filter(Boolean),
      request_compression: values.request_compression,
      test_model: values.test_model.trim() === "" ? null : values.test_model.trim(),
      test_pricing_model_id:
        values.test_pricing_model_id === "" ? null : values.test_pricing_model_id,
      auto_disable_allowed: values.auto_disable_allowed,
    };
    const input: ChannelCapabilityInput = {
      channel_id: values.channel_id,
      settings,
      status_statistics_enabled: values.status_statistics_enabled,
      config_template_id: values.config_template_id === "" ? null : values.config_template_id,
      override_document: JSON.parse(values.override_document) as unknown,
      billing_multiplier: values.billing_multiplier,
    };
    try {
      if (isNew) {
        const result = await create.mutateAsync(input);
        navigate(`/admin/routing/capabilities/${result.id}`, { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
      }
      toast.success(t("Capability saved"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This capability was changed elsewhere. Reloading."));
        await query.refetch();
      } else {
        toast.error(t(controlPlaneMutationErrorMessage(error, "Could not save capability.")));
      }
    }
  });

  const confirmDelete = async () => {
    try {
      await remove.mutateAsync({ ifMatch: query.etag });
      toast.success(t("Capability deleted"));
      navigate(`/admin/routing/channels?channel=${form.getValues("channel_id")}`);
    } catch (error) {
      toast.error(t(controlPlaneMutationErrorMessage(error, "Could not delete capability.")));
    }
  };

  const confirmRecovery = async () => {
    try {
      await recover.mutateAsync({ ifMatch: query.etag });
      setConfirmingRecovery(false);
      toast.success(t("Capability recovered"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This capability was changed elsewhere. Reloading."));
        await query.refetch();
      } else {
        toast.error(t(controlPlaneMutationErrorMessage(error, "Could not recover capability.")));
      }
    }
  };

  const transports = form.watch("transports");
  const toggleTransport = (transport: CapabilityTransport, checked: boolean) => {
    const next = checked
      ? [...transports, transport]
      : transports.filter((value) => value !== transport);
    form.setValue("transports", next, { shouldDirty: true });
  };

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New capability") : t("Channel capability")}
        description={t(
          "Only connector-implemented operation/transport combinations are accepted. Saving never grants API key access.",
        )}
        backPath={`/admin/routing/channels?channel=${form.watch("channel_id")}`}
        isLoading={!isNew && query.isLoading}
        error={query.error}
        hasData={isNew || Boolean(capability)}
        saving={busy}
        editCard={
          <Card>
            <CardHeader>
              <CardTitle>{t("Capability settings")}</CardTitle>
              <CardDescription>
                {t("Declared here; enabled, routed, and granted separately.")}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {capability?.auto_disabled && (
                <Alert className="mb-4">
                  <AlertTitle>{t("Automatically disabled")}</AlertTitle>
                  <AlertDescription>
                    <p>{capability.auto_disable_reason}</p>
                    <Button variant="outline" disabled={busy} onClick={() => setConfirmingRecovery(true)}>
                      {t("Recover capability")}
                    </Button>
                  </AlertDescription>
                </Alert>
              )}
              <form onSubmit={submit} className="flex flex-col gap-5">
                <FieldGroup>
                  <Field data-disabled={!isNew} data-invalid={Boolean(form.formState.errors.channel_id)}>
                    <FieldLabel htmlFor="capability-channel">{t("Logical channel")}</FieldLabel>
                    <Select
                      disabled={!isNew}
                      value={form.watch("channel_id") || NONE}
                      onValueChange={(value) =>
                        form.setValue("channel_id", value === NONE ? "" : value, {
                          shouldDirty: true,
                        })
                      }
                    >
                      <SelectTrigger
                        id="capability-channel"
                        aria-invalid={Boolean(form.formState.errors.channel_id)}
                      >
                        <SelectValue placeholder={t("Pick a channel")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value={NONE}>{t("Choose a channel")}</SelectItem>
                          {channels.data?.map((channel) => (
                            <SelectItem key={channel.id} value={channel.id}>
                              {channelNames.get(channel.id) ?? channel.name}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldError errors={[form.formState.errors.channel_id]} />
                  </Field>
                  <Field data-disabled={!isNew}>
                    <FieldLabel htmlFor="capability-operation">{t("Operation")}</FieldLabel>
                    <Select
                      disabled={!isNew}
                      value={form.watch("operation")}
                      onValueChange={(value) =>
                        form.setValue("operation", value as ApiOperation, { shouldDirty: true })
                      }
                    >
                      <SelectTrigger id="capability-operation">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          {API_OPERATIONS.map((operation) => (
                            <SelectItem key={operation} value={operation}>
                              {apiOperationLabel(operation)}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </Field>
                  <FieldSet data-invalid={Boolean(form.formState.errors.transports)}>
                    <FieldLegend>{t("Transports")}</FieldLegend>
                    <FieldDescription>
                      {t("Only combinations implemented by the channel connector are accepted.")}
                    </FieldDescription>
                    <div className="flex flex-wrap gap-4">
                      {CAPABILITY_TRANSPORTS.map((transport) => (
                        <label
                          key={transport}
                          className="flex items-center gap-2 text-sm"
                          htmlFor={`capability-transport-${transport}`}
                        >
                          <Checkbox
                            id={`capability-transport-${transport}`}
                            checked={transports.includes(transport)}
                            onCheckedChange={(checked) =>
                              toggleTransport(transport, checked === true)
                            }
                          />
                          {capabilityTransportLabel(transport)}
                        </label>
                      ))}
                    </div>
                    <FieldError errors={[form.formState.errors.transports]} />
                  </FieldSet>
                  <StringListField
                    id="capability-models"
                    label={t("Available models")}
                    description={t("Upstream wire model identifiers, one per line.")}
                    value={form.watch("available_models")}
                    onChange={(value) =>
                      form.setValue("available_models", value, { shouldDirty: true })
                    }
                  />
                  {selectedAccess?.connector_kind === "openai_compatible" && (
                    <Button type="button" variant="outline" disabled={discover.isPending}
                      onClick={() => void discoverModels()}>
                      {t("Fetch models")}
                    </Button>
                  )}
                  <ChannelModelPickerDialog
                    open={pickingModels} onOpenChange={setPickingModels}
                    models={discover.data?.models ?? []}
                    currentModels={form.watch("available_models")}
                    onApply={(models) => form.setValue("available_models", models, { shouldDirty: true })}
                  />
                  <Field>
                    <FieldLabel htmlFor="capability-compression">
                      {t("Request compression")}
                    </FieldLabel>
                    <Select
                      value={form.watch("request_compression")}
                      onValueChange={(value) =>
                        form.setValue("request_compression", value as RequestCompression, {
                          shouldDirty: true,
                        })
                      }
                    >
                      <SelectTrigger id="capability-compression">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          {REQUEST_COMPRESSIONS.map((compression) => (
                            <SelectItem key={compression} value={compression}>
                              {compression === "default" ? t("Default") : "Zstandard (zstd)"}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldDescription>
                      {t("Zstandard applies only to Responses HTTP JSON/SSE.")}
                    </FieldDescription>
                  </Field>
                  <Field orientation="horizontal">
                    <FieldLabel htmlFor="capability-enabled">{t("Enabled")}</FieldLabel>
                    <Switch
                      id="capability-enabled"
                      checked={form.watch("enabled")}
                      onCheckedChange={(value) =>
                        form.setValue("enabled", value, { shouldDirty: true })
                      }
                    />
                  </Field>
                  <Field orientation="horizontal">
                    <FieldLabel htmlFor="capability-auto-disable">
                      {t("Allow automatic disable")}
                    </FieldLabel>
                    <Switch
                      id="capability-auto-disable"
                      checked={form.watch("auto_disable_allowed")}
                      onCheckedChange={(value) =>
                        form.setValue("auto_disable_allowed", value, { shouldDirty: true })
                      }
                    />
                  </Field>
                  <Field orientation="horizontal">
                    <FieldLabel htmlFor="capability-statistics">
                      {t("Status statistics")}
                    </FieldLabel>
                    <Switch
                      id="capability-statistics"
                      checked={form.watch("status_statistics_enabled")}
                      onCheckedChange={(value) =>
                        form.setValue("status_statistics_enabled", value, { shouldDirty: true })
                      }
                    />
                  </Field>
                  <Field>
                    <FieldLabel htmlFor="capability-template">{t("Config template")}</FieldLabel>
                    <Select
                      value={form.watch("config_template_id") || NONE}
                      onValueChange={(value) =>
                        form.setValue("config_template_id", value === NONE ? "" : value, {
                          shouldDirty: true,
                        })
                      }
                    >
                      <SelectTrigger id="capability-template">
                        <SelectValue placeholder={t("No template")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value={NONE}>{t("No template")}</SelectItem>
                          {templates.data?.map((template) => (
                            <SelectItem key={template.id} value={template.id}>
                              {template.name}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </Field>
                  <DecimalField
                    id="capability-multiplier"
                    label={t("Billing multiplier")}
                    value={form.watch("billing_multiplier")}
                    onChange={(value) =>
                      form.setValue("billing_multiplier", value, { shouldDirty: true })
                    }
                    error={form.formState.errors.billing_multiplier?.message}
                  />
                  <Field
                    data-invalid={Boolean(form.formState.errors.override_document)}
                  >
                    <FieldLabel htmlFor="capability-overrides">
                      {t("Override document")}
                    </FieldLabel>
                    <Textarea
                      id="capability-overrides"
                      rows={6}
                      className="font-mono text-xs"
                      {...form.register("override_document")}
                      aria-invalid={Boolean(form.formState.errors.override_document)}
                    />
                    <FieldDescription>
                      {t("Capability-level JSON transform overrides. Must be a JSON object.")}
                    </FieldDescription>
                    <FieldError errors={[form.formState.errors.override_document]} />
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.test_model)}>
                    <FieldLabel htmlFor="capability-test-model">{t("Probe test model")}</FieldLabel>
                    <Input
                      id="capability-test-model"
                      {...form.register("test_model")}
                      aria-invalid={Boolean(form.formState.errors.test_model)}
                    />
                  </Field>
                  <Field
                    data-invalid={Boolean(form.formState.errors.test_pricing_model_id)}
                  >
                    <FieldLabel htmlFor="capability-test-price">
                      {t("Probe pricing model")}
                    </FieldLabel>
                    <Input
                      id="capability-test-price"
                      {...form.register("test_pricing_model_id")}
                      aria-invalid={Boolean(form.formState.errors.test_pricing_model_id)}
                    />
                    <FieldDescription>
                      {t("Both probe fields are set or cleared together.")}
                    </FieldDescription>
                    <FieldError errors={[form.formState.errors.test_pricing_model_id]} />
                  </Field>
                </FieldGroup>
                <Button type="submit" disabled={busy}>
                  {t("Save capability")}
                </Button>
              </form>
            </CardContent>
          </Card>
        }
        dangerZone={
          isNew ? undefined : (
            <div className="flex flex-col gap-3">
              <p className="text-sm text-muted-foreground">
                {t("Routing candidates referencing this capability must be withdrawn first.")}
              </p>
              <Button
                type="button"
                variant="destructive"
                className="self-start"
                disabled={busy}
                onClick={() => setConfirmingDelete(true)}
              >
                {t("Delete capability")}
              </Button>
            </div>
          )
        }
      />
      <ConfirmDialog
        open={confirmingDelete}
        onOpenChange={setConfirmingDelete}
        title={t("Delete this capability?")}
        description={t("The capability is soft-deleted after dependent routes are withdrawn.")}
        destructive
        confirmDisabled={busy}
        onConfirm={confirmDelete}
      />
      <ConfirmDialog
        open={confirmingRecovery}
        onOpenChange={setConfirmingRecovery}
        title={t("Recover capability")}
        description={t("Clear automatic-disable state without changing the explicit enabled setting or API key grants.")}
        confirmLabel={t("Recover")}
        onConfirm={confirmRecovery}
        confirmDisabled={recover.isPending}
      />
    </>
  );
}
