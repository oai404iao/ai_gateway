import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
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
import { ModelRuleTierEditor } from "./model-rule-tier-editor";
import {
  safeAdminReturnPath,
  validResourceId,
} from "@/features/admin/model-setup/model-setup-navigation";

const channelWeightSchema = z.object({
  channel_id: z.string().min(1),
  weight: z.number().int("Channel weights must be whole numbers.").min(
    1,
    "Channel weights must be positive.",
  ).max(2_147_483_647, "Channel weights are too large."),
});

const groupTargetSchema = z
  .object({
    channel_group_id: z.string().min(1),
    channel_selection: z.enum(["all", "selected"]),
    default_weight: z
      .number()
      .int("Default weight must be a whole number.")
      .min(1, "Default weight must be positive.")
      .max(2_147_483_647, "Default weight is too large.")
      .nullable(),
    channels: z.array(channelWeightSchema),
  })
  .superRefine((value, context) => {
    if (value.channel_selection === "all" && value.default_weight === null) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["default_weight"],
        message: "All-channel routing requires a positive default weight.",
      });
    }
    if (value.channel_selection === "selected" && value.default_weight !== null) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["default_weight"],
        message: "Selected-channel routing cannot have a default weight.",
      });
    }
    if (value.channel_selection === "selected" && value.channels.length === 0) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["channels"],
        message: "Select at least one channel and assign a positive weight.",
      });
    }
    const channelIds = new Set<string>();
    value.channels.forEach((channel, channelIndex) => {
      if (channelIds.has(channel.channel_id)) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["channels", channelIndex, "channel_id"],
          message: "A channel can appear only once in a group target.",
        });
      }
      channelIds.add(channel.channel_id);
    });
  });

const routingTierSchema = z.object({
  priority: z.number().int("Priority must be a whole number.").min(
    0,
    "Priority must be zero or greater.",
  ).max(2_147_483_647, "Priority is too large."),
  selection_strategy: z.enum(["weighted_random", "weighted_round_robin"]),
  channel_groups: z
    .array(groupTargetSchema)
    .min(1, "Add at least one channel group to this tier."),
});

const schema = z
  .object({
    client_model: z.string().min(1, "Client model is required."),
    api_format: z.enum([
      "open_ai_chat_completions",
      "open_ai_responses",
      "open_ai_images",
    ]),
    upstream_model_id: z.string().min(1, "Pick an upstream model."),
    description: z.string().nullable(),
    routing_tiers: z
      .array(routingTierSchema)
      .min(1, "Add at least one routing tier."),
    enabled: z.boolean(),
  })
  .superRefine((value, context) => {
    const priorities = new Set<number>();
    const groupIds = new Set<string>();
    value.routing_tiers.forEach((tier, tierIndex) => {
      if (priorities.has(tier.priority)) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["routing_tiers", tierIndex, "priority"],
          message: "Tier priorities must be unique.",
        });
      }
      priorities.add(tier.priority);
      tier.channel_groups.forEach((target, targetIndex) => {
        if (groupIds.has(target.channel_group_id)) {
          context.addIssue({
            code: z.ZodIssueCode.custom,
            path: [
              "routing_tiers",
              tierIndex,
              "channel_groups",
              targetIndex,
              "channel_group_id",
            ],
            message: "A channel group can appear only once in a rule.",
          });
        }
        groupIds.add(target.channel_group_id);
      });
    });
  });

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  client_model: "",
  api_format: "open_ai_chat_completions",
  upstream_model_id: "",
  description: null,
  routing_tiers: [
    {
      priority: 0,
      selection_strategy: "weighted_random",
      channel_groups: [],
    },
  ],
  enabled: true,
};

const CUSTOM_CLIENT_MODEL = "__custom_client_model__";

export function ModelRuleDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const returnTo = safeAdminReturnPath(
    searchParams.get("returnTo"),
    "/admin/routing/model-rules",
  );
  const returnsToSetup = returnTo.startsWith("/admin/model-setup");
  const preferredModelId = isNew
    ? validResourceId(searchParams.get("upstreamModelId"))
    : null;
  const preferredGroupId = isNew
    ? validResourceId(searchParams.get("channelGroupId"))
    : null;
  const preferredChannelId = isNew
    ? validResourceId(searchParams.get("channelId"))
    : null;
  const preferredClientModel = isNew
    ? searchParams.get("clientModel")?.trim() || null
    : null;
  const requestedApiFormat = searchParams.get("apiFormat");
  const preferredApiFormat: ApiFormat | null = API_FORMATS.includes(
    requestedApiFormat as ApiFormat,
  )
    ? (requestedApiFormat as ApiFormat)
    : null;
  const { data, etag, isLoading, error } = useModelRule(id);
  const create = useCreateModelRule();
  const update = useUpdateModelRule(id);
  const models = useModels();
  const groups = useChannelGroups();
  const channels = useChannels();
  const { t } = useI18n();
  const hasPrefill = Boolean(
    preferredModelId ||
      preferredGroupId ||
      preferredChannelId ||
      preferredClientModel ||
      preferredApiFormat,
  );
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [prefillInitialized, setPrefillInitialized] = useState(!hasPrefill);
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
        routing_tiers: data.data.routing_tiers,
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  useEffect(() => {
    if (!isNew || prefillInitialized || !hasPrefill) return;
    if (
      (preferredModelId && !models.data) ||
      (preferredGroupId && !groups.data) ||
      (preferredGroupId && !channels.data) ||
      (preferredChannelId && !channels.data)
    ) {
      return;
    }

    const model = preferredModelId
      ? models.data?.find((candidate) => candidate.id === preferredModelId)
      : undefined;
    const group = preferredGroupId
      ? groups.data?.find((candidate) => candidate.id === preferredGroupId)
      : undefined;
    const channel = preferredChannelId
      ? channels.data?.find(
          (candidate) => candidate.id === preferredChannelId,
        )
      : undefined;
    const apiFormat =
      preferredApiFormat ??
      group?.api_format ??
      channel?.api_format ??
      empty.api_format;
    const selectedModel = model ?? models.data?.find(
      (candidate) => candidate.id === state.upstream_model_id,
    );
    const groupChannels = group
      ? (channels.data ?? []).filter(
          (candidate) =>
            candidate.channel_group_id === group.id &&
            candidate.api_format === apiFormat &&
            candidate.enabled,
        )
      : [];
    const compatibleGroupChannels = selectedModel
      ? groupChannels.filter((candidate) =>
          candidate.available_models.includes(selectedModel.source_model_id),
        )
      : groupChannels;
    const partiallyCompatible =
      Boolean(selectedModel) &&
      compatibleGroupChannels.length > 0 &&
      compatibleGroupChannels.length < groupChannels.length;
    const noCompatibleChannels =
      Boolean(selectedModel) &&
      groupChannels.length > 0 &&
      compatibleGroupChannels.length === 0;
    const groupTarget =
      group?.api_format === apiFormat && !noCompatibleChannels
        ? {
            channel_group_id: group.id,
            channel_selection: partiallyCompatible
              ? ("selected" as const)
              : ("all" as const),
            default_weight: partiallyCompatible ? null : 100,
            channels: partiallyCompatible
              ? compatibleGroupChannels.map((candidate) => ({
                  channel_id: candidate.id,
                  weight: 100,
                }))
              : [],
          }
        : null;
    const channelTarget =
      channel?.api_format === apiFormat
        ? {
            channel_group_id: channel.channel_group_id,
            channel_selection: "selected" as const,
            default_weight: null,
            channels: [{ channel_id: channel.id, weight: 100 }],
          }
        : null;
    setState((current) => ({
      ...current,
      client_model:
        preferredClientModel ??
        model?.source_model_id ??
        current.client_model,
      api_format: apiFormat,
      upstream_model_id: model?.id ?? current.upstream_model_id,
      routing_tiers: [
        {
          priority: 0,
          selection_strategy: "weighted_random",
          channel_groups: channelTarget
            ? [channelTarget]
            : groupTarget
              ? [groupTarget]
              : [],
        },
      ],
    }));
    setPrefillInitialized(true);
  }, [
    channels.data,
    groups.data,
    hasPrefill,
    isNew,
    models.data,
    prefillInitialized,
    preferredApiFormat,
    preferredChannelId,
    preferredClientModel,
    preferredGroupId,
    preferredModelId,
    state.upstream_model_id,
  ]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const clientModelSelection = useMemo(
    () =>
      models.data?.some((model) => model.source_model_id === state.client_model)
        ? state.client_model
        : CUSTOM_CLIENT_MODEL,
    [models.data, state.client_model],
  );
  const targetGroups = useMemo(
    () =>
      (groups.data ?? []).filter(
        (group) => group.api_format === state.api_format,
      ),
    [groups.data, state.api_format],
  );
  const targetChannels = useMemo(
    () =>
      (channels.data ?? []).filter(
        (channel) => channel.api_format === state.api_format,
      ),
    [channels.data, state.api_format],
  );

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
      routing_tiers: parsed.data.routing_tiers,
      enabled: parsed.data.enabled,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("Model rule created"));
        navigate(returnTo, { replace: true });
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

  const fieldError = (path: string | Array<string | number>) => {
    const normalizedPath = Array.isArray(path) ? path.join(".") : path;
    const message = validation?.issues.find(
      (issue) => issue.path.join(".") === normalizedPath,
    )?.message;
    return message ? t(message) : undefined;
  };
  const prefillError =
    (preferredModelId ? models.error : null) ??
    (preferredGroupId ? groups.error : null) ??
    (preferredGroupId ? channels.error : null) ??
    (preferredChannelId ? channels.error : null);
  const prefillLoading =
    hasPrefill &&
    !prefillError &&
    ((Boolean(preferredModelId) && models.isLoading) ||
      (Boolean(preferredGroupId) && groups.isLoading) ||
      (Boolean(preferredGroupId) && channels.isLoading) ||
      (Boolean(preferredChannelId) && channels.isLoading) ||
      (!prefillInitialized && !prefillError));

  return (
    <AdminDetailShell
      title={isNew ? t("New model rule") : state.client_model || t("Model Rules")}
      description={t(
        "Routes a client model and API format through rule-owned priority tiers to one priced upstream model.",
      )}
      backPath={returnTo}
      backLabel={t(returnsToSetup ? "Back to model setup" : "Back to rules")}
      isLoading={isLoading || prefillLoading}
      error={error ?? prefillError}
      hasData={isNew ? prefillInitialized : Boolean(data)}
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
                <DetailField
                  label={t("Routing tiers")}
                  value={t("{count} tiers · priorities {priorities}", {
                    count: data.data.routing_tiers.length,
                    priorities: data.data.routing_tiers
                      .map((tier) => tier.priority)
                      .join(", "),
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
                      patch({
                        api_format: value as ApiFormat,
                        routing_tiers: [
                          {
                            priority: 0,
                            selection_strategy: "weighted_random",
                            channel_groups: [],
                          },
                        ],
                      })
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
                <ModelRuleTierEditor
                  className="xl:col-span-2"
                  value={state.routing_tiers}
                  groups={targetGroups}
                  channels={targetChannels}
                  onChange={(routingTiers) =>
                    patch({ routing_tiers: routingTiers })
                  }
                  errorFor={fieldError}
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
