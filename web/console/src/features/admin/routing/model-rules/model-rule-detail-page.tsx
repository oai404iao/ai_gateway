import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import {
  RoutingTargetFields,
  type RoutingTargetChannel,
  type RoutingTargetGroup,
} from "@/components/shared/routing-target-fields";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useChannelGroups,
  useChannels,
  useCreateModelRule,
  useModelRule,
  useModels,
  useUpdateModelRule,
} from "@/features/admin/api";
import { groupModelsByProvider } from "@/features/admin/models/model-groups";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ApiFormat, ModelRuleInput } from "@/api/types";
import { API_FORMATS, apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const schema = z.object({
  client_model: z.string().min(1, "Client model is required."),
  api_format: z.enum(["open_ai_chat_completions", "open_ai_responses", "open_ai_images"]),
  upstream_model_id: z.string().min(1, "Pick an upstream model."),
  description: z.string().nullable(),
  channel_group_ids: z.array(z.string()),
  channel_ids: z.array(z.string()),
  enabled: z.boolean(),
}).superRefine((value, context) => {
  if (value.channel_group_ids.length === 0 && value.channel_ids.length === 0) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["channel_group_ids"],
      message: "Pick at least one channel group or channel.",
    });
  }
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  client_model: "",
  api_format: "open_ai_chat_completions",
  upstream_model_id: "",
  description: null,
  channel_group_ids: [],
  channel_ids: [],
  enabled: true,
};

const CUSTOM_CLIENT_MODEL = "__custom_client_model__";

export function ModelRuleDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useModelRule(id);
  const create = useCreateModelRule();
  const update = useUpdateModelRule(id);
  const models = useModels();
  const groups = useChannelGroups();
  const channels = useChannels();
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const modelProviderGroups = useMemo(
    () => groupModelsByProvider(models.data ?? [], t("Unspecified provider")),
    [models.data, t],
  );

  useEffect(() => {
    if (data) {
      setState({
        client_model: data.data.client_model,
        api_format: data.data.api_format,
        upstream_model_id: data.data.upstream_model_id,
        description: data.data.description,
        channel_group_ids: data.data.channel_group_ids,
        channel_ids: data.data.channel_ids,
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const selectedUpstreamModel = useMemo(
    () => models.data?.find((model) => model.id === state.upstream_model_id),
    [models.data, state.upstream_model_id],
  );
  const clientModelSelection = useMemo(
    () =>
      models.data?.some((model) => model.source_model_id === state.client_model)
        ? state.client_model
        : CUSTOM_CLIENT_MODEL,
    [models.data, state.client_model],
  );
  const targetGroups = useMemo<RoutingTargetGroup[]>(
    () =>
      (groups.data ?? [])
        .filter((group) => group.api_format === state.api_format)
        .map((group) => ({
          id: group.id,
          name: group.name,
          api_format: group.api_format,
          enabled: group.enabled,
          priority: group.priority,
          model_capable: selectedUpstreamModel
            ? (channels.data ?? []).some(
                (channel) =>
                  channel.channel_group_id === group.id &&
                  channel.available_models.includes(
                    selectedUpstreamModel.source_model_id,
                  ),
              )
            : undefined,
        })),
    [channels.data, groups.data, selectedUpstreamModel, state.api_format],
  );
  const targetChannels = useMemo<RoutingTargetChannel[]>(() => {
    const groupById = new Map((groups.data ?? []).map((group) => [group.id, group]));
    return (channels.data ?? [])
      .filter((channel) => channel.api_format === state.api_format)
      .map((channel) => {
        const group = groupById.get(channel.channel_group_id);
        return {
          id: channel.id,
          channel_group_id: channel.channel_group_id,
          channel_group_name: group?.name,
          channel_group_enabled: group?.enabled ?? false,
          name: channel.name,
          api_format: channel.api_format,
          enabled: channel.enabled,
          auto_disabled: channel.auto_disabled,
          model_capable: selectedUpstreamModel
            ? channel.available_models.includes(
                selectedUpstreamModel.source_model_id,
              )
            : undefined,
        };
      });
  }, [
    channels.data,
    groups.data,
    selectedUpstreamModel,
    state.api_format,
  ]);

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    const input: ModelRuleInput = {
      client_model: parsed.data.client_model,
      api_format: parsed.data.api_format as ApiFormat,
      upstream_model_id: parsed.data.upstream_model_id,
      description: parsed.data.description,
      channel_group_ids: parsed.data.channel_group_ids,
      channel_ids: parsed.data.channel_ids,
      enabled: parsed.data.enabled,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("Model rule created"));
        navigate("/admin/routing/model-rules", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Model rule updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This rule was changed elsewhere. Reloading."));
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
    <AdminDetailShell
      title={isNew ? t("New model rule") : state.client_model || t("Model Rules")}
      description={t("Routes a client model and API format to one priced upstream model and channels.")}
      backPath="/admin/routing/model-rules"
      backLabel={t("Back to rules")}
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.client_model}</CardTitle>
              <CardDescription>{apiFormatLabel(data.data.api_format)}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField
                  label={t("Upstream model")}
                  value={data.data.upstream_model}
                  mono
                />
                <DetailField
                  label={t("Enabled")}
                  value={<StatusBadge value={data.data.enabled} />}
                />
                <DetailField
                  label={t("Routing status")}
                  value={<StatusBadge value={data.data.routing_status} />}
                />
                <DetailField
                  label={t("Routing candidates")}
                  value={t("Active {active} · Capable {capable} · Targets {target}", {
                    active: data.data.active_channel_count,
                    capable: data.data.model_capable_channel_count,
                    target: data.data.target_channel_count,
                  })}
                />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? t("Create rule") : t("Edit rule")}</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup className="grid gap-5 xl:grid-cols-2">
                <Field data-invalid={Boolean(fieldError("client_model"))}>
                  <FieldLabel>{t("Client model")}</FieldLabel>
                  <Select
                    value={clientModelSelection}
                    onValueChange={(value) =>
                      patch({ client_model: value === CUSTOM_CLIENT_MODEL ? "" : value })
                    }
                  >
                    <SelectTrigger
                      aria-label={t("Client model")}
                      aria-invalid={Boolean(fieldError("client_model"))}
                    >
                      <SelectValue placeholder={t("Pick a client model")} />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value={CUSTOM_CLIENT_MODEL}>
                          {t("Custom client model")}
                        </SelectItem>
                      </SelectGroup>
                      {modelProviderGroups.map((providerGroup) => (
                        <SelectGroup key={providerGroup.provider}>
                          <SelectLabel>{providerGroup.provider}</SelectLabel>
                          {providerGroup.models.map((model) => (
                            <SelectItem key={model.id} value={model.source_model_id}>
                              {model.display_name} ({model.source_model_id})
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      ))}
                    </SelectContent>
                  </Select>
                  {clientModelSelection === CUSTOM_CLIENT_MODEL ? (
                    <Input
                      id="client_model"
                      value={state.client_model}
                      onChange={(event) => patch({ client_model: event.target.value })}
                      placeholder={t("Enter a custom client model")}
                      aria-label={t("Custom client model")}
                      aria-invalid={Boolean(fieldError("client_model"))}
                    />
                  ) : null}
                  <FieldDescription>
                    {t("Choose an upstream model or use Custom client model to enter an alias.")}
                  </FieldDescription>
                  {fieldError("client_model") ? (
                    <FieldError>{fieldError("client_model")}</FieldError>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel>{t("API format")}</FieldLabel>
                  <Select
                    value={state.api_format}
                    onValueChange={(value) =>
                      patch({ api_format: value as ApiFormat, channel_group_ids: [], channel_ids: [] })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {API_FORMATS.map((format) => (
                          <SelectItem key={format} value={format}>
                            {apiFormatLabel(format)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field data-invalid={Boolean(fieldError("upstream_model_id"))}>
                  <FieldLabel>{t("Upstream model")}</FieldLabel>
                  <Select
                    value={state.upstream_model_id || "__none__"}
                    onValueChange={(value) =>
                      patch({ upstream_model_id: value === "__none__" ? "" : value })
                    }
                  >
                    <SelectTrigger aria-invalid={Boolean(fieldError("upstream_model_id"))}>
                      <SelectValue placeholder={t("Pick an upstream model")} />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="__none__">{t("None")}</SelectItem>
                      </SelectGroup>
                      {modelProviderGroups.map((providerGroup) => (
                        <SelectGroup key={providerGroup.provider}>
                          <SelectLabel>{providerGroup.provider}</SelectLabel>
                          {providerGroup.models.map((model) => (
                            <SelectItem key={model.id} value={model.id}>
                              {model.display_name} ({model.source_model_id})
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      ))}
                    </SelectContent>
                  </Select>
                  {fieldError("upstream_model_id") ? (
                    <FieldError>{fieldError("upstream_model_id")}</FieldError>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="description">{t("Description")}</FieldLabel>
                  <Input
                    id="description"
                    value={state.description ?? ""}
                    onChange={(event) => patch({ description: event.target.value || null })}
                  />
                </Field>
                <RoutingTargetFields
                  className="xl:col-span-2"
                  groups={targetGroups}
                  channels={targetChannels}
                  selectedGroupIds={state.channel_group_ids}
                  selectedChannelIds={state.channel_ids}
                  onChange={(channelGroupIds, channelIds) =>
                    patch({
                      channel_group_ids: channelGroupIds,
                      channel_ids: channelIds,
                    })
                  }
                  error={
                    fieldError("channel_group_ids") ?? fieldError("channel_ids")
                  }
                  allowUnavailableSelection
                />
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="model_rule_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="model_rule_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create rule") : t("Save rule")}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
