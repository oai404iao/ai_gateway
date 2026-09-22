import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
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
  useChannelCapabilities,
  useCreateOperationRule,
  useLogicalChannels,
  useOperationRule,
  useOperationRules,
  useUpdateOperationRule,
  useRoutingProfiles,
  useCreateRoutingProfile,
  useModels,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { API_OPERATIONS, apiOperationLabel } from "@/lib/permissions";
import type { ApiOperation, OperationRuleInput, OperationTierInput } from "@/api/types";
import { OperationRuleTierEditor } from "./operation-rule-tier-editor";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const candidateSchema = z.object({
  capability_id: z.string().regex(UUID, "Choose a capability."),
  upstream_model: z
    .string()
    .trim()
    .min(1, "Choose an upstream model.")
    .max(300, "Upstream model must be at most 300 characters."),
  weight: z.number().int().min(1).max(2_147_483_647),
});

const tierSchema = z
  .object({
    priority: z.number().int().min(0).max(2_147_483_647),
    selection_strategy: z.enum(["weighted_random", "weighted_round_robin"]),
    candidates: z.array(candidateSchema).min(1),
  })
  .superRefine((value, context) => {
    const pairs = new Set<string>();
    value.candidates.forEach((candidate, index) => {
      const key = `${candidate.capability_id}\u0000${candidate.upstream_model}`;
      if (pairs.has(key)) {
        context.addIssue({
          code: "custom",
          path: ["candidates", index, "upstream_model"],
          message: "A capability and upstream-model pair can appear only once in a tier.",
        });
      }
      pairs.add(key);
    });
  });

const schema = z
  .object({
    model_routing_profile_id: z.string().regex(UUID, "Enter a valid routing profile ID."),
    operation: z.enum([
      "chat_completion",
      "responses",
      "responses-ws",
      "web_search",
      "images_generation",
      "images_edit",
    ]),
    enabled: z.boolean(),
    routing_tiers: z.array(tierSchema),
  })
  .superRefine((value, context) => {
    if (value.enabled && value.routing_tiers.length === 0) {
      context.addIssue({
        code: "custom",
        path: ["routing_tiers"],
        message: "Add a priority tier before enabling this rule.",
      });
    }
    const priorities = new Set<number>();
    value.routing_tiers.forEach((tier, tierIndex) => {
      if (priorities.has(tier.priority)) {
        context.addIssue({
          code: "custom",
          path: ["routing_tiers", tierIndex, "priority"],
          message: "Tier priorities must be unique.",
        });
      }
      priorities.add(tier.priority);
    });
  });

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  model_routing_profile_id: "",
  operation: "responses",
  enabled: false,
  routing_tiers: [],
};

export function OperationRuleDetailPage({
  ruleId,
  modelId,
  onCreated,
  embedded = false,
}: {
  ruleId?: string;
  modelId?: string;
  onCreated?: (id: string) => void;
  embedded?: boolean;
} = {}) {
  const { id: pathId = "" } = useParams();
  const id = ruleId ?? pathId;
  const isNew = id === "new";
  const navigate = useNavigate();
  const { t } = useI18n();
  const query = useOperationRule(id);
  const capabilities = useChannelCapabilities();
  const channels = useLogicalChannels();
  const profiles = useRoutingProfiles();
  const models = useModels();
  const rules = useOperationRules();
  const createProfile = useCreateRoutingProfile();
  const [newProfileModel, setNewProfileModel] = useState("");
  const [createdProfileId, setCreatedProfileId] = useState("");
  const create = useCreateOperationRule();
  const update = useUpdateOperationRule(id);
  const [state, setState] = useState<FormState>(empty);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const rule = query.data?.data;
  const busy = submitting || create.isPending || update.isPending || createProfile.isPending;
  const draft = useConfigurationDraft(busy);

  const addProfile = async () => {
    try {
      const result = await createProfile.mutateAsync({ model_id: newProfileModel });
      await profiles.refetch();
      setState((current) => ({ ...current, model_routing_profile_id: result.id }));
      setNewProfileModel("");
      toast.success(t("Routing profile created"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Save failed"));
    }
  };

  useEffect(() => {
    if (!rule) return;
    setState({
      model_routing_profile_id: rule.model_routing_profile_id,
      operation: rule.operation,
      enabled: rule.enabled,
      routing_tiers: rule.routing_tiers,
    });
  }, [rule]);

  const modelProfile = profiles.data?.find((profile) => profile.model_id === modelId);
  const modelProfileId = modelProfile?.id ?? createdProfileId;
  const availableOperations = API_OPERATIONS.filter((operation) =>
    !modelId || !(rules.data ?? []).some((existing) =>
      existing.model_routing_profile_id === modelProfileId &&
      existing.operation === operation,
    ),
  );
  useEffect(() => {
    if (!isNew || !modelId) return;
    const operations = API_OPERATIONS.filter((operation) =>
      !(rules.data ?? []).some((existing) =>
        existing.model_routing_profile_id === modelProfileId && existing.operation === operation,
      ),
    );
    setState((current) => ({
      ...current,
      model_routing_profile_id: modelProfileId,
      operation: draft.dirty || operations.includes(current.operation)
        ? current.operation : operations[0] ?? current.operation,
    }));
  }, [isNew, modelId, modelProfileId, rules.data, draft.dirty]);

  const channelNames = useMemo(
    () => new Map((channels.data ?? []).map((channel) => [channel.id, channel.name])),
    [channels.data],
  );
  const operationCapabilities = useMemo(
    () => (capabilities.data ?? []).filter((capability) => capability.settings.operation === state.operation),
    [capabilities.data, state.operation],
  );

  const patch = (partial: Partial<FormState>) => {
    draft.markDirty();
    setState((current) => ({ ...current, ...partial }));
  };
  const fieldError = (path: string | Array<string | number>) => {
    const normalized = Array.isArray(path) ? path.join(".") : path;
    const message = validation?.issues.find(
      (issue) => issue.path.join(".") === normalized,
    )?.message;
    return message ? t(message) : undefined;
  };

  const submit = async () => {
    if (isNew && modelId && !availableOperations.includes(state.operation)) {
      setValidation(new z.ZodError([{
        code: "custom",
        path: ["operation"],
        message: "This operation is already configured. Choose another operation.",
      }]));
      return;
    }
    const parsed = schema.safeParse({
      ...state,
      model_routing_profile_id: modelId && !modelProfileId
        ? modelId
        : state.model_routing_profile_id,
    });
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    try {
      const input: OperationRuleInput = parsed.data;
      if (isNew && modelId && !modelProfileId) {
        const profile = await createProfile.mutateAsync({ model_id: modelId });
        setCreatedProfileId(profile.id);
        input.model_routing_profile_id = profile.id;
        setState((current) => ({ ...current, model_routing_profile_id: profile.id }));
        await profiles.refetch();
      }
      if (isNew) {
        const result = await create.mutateAsync(input);
        await rules.refetch();
        toast.success(t("Operation rule created"));
        draft.markSaved();
        if (onCreated) onCreated(result.id);
        else navigate(`/admin/routing/operation-rules/${result.id}`, { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
        draft.markSaved();
        toast.success(t("Operation rule saved"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This rule was changed elsewhere. Reloading."));
        if (isNew) await Promise.all([profiles.refetch(), rules.refetch()]);
        else await query.refetch();
      } else {
        toast.error(t(controlPlaneMutationErrorMessage(error, "Could not save operation rule.")));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const routingTiers: OperationTierInput[] = state.routing_tiers;

  return (
    <AdminDetailShell
      embedded={embedded}
      navigationGuard={draft.navigationGuard}
      title={isNew ? t("New operation rule") : t("Operation rule")}
      description={t(
        "Candidates reference an explicit capability and upstream model. Tiers, priorities, and weights are never inferred from legacy channels.",
      )}
      backPath="/admin/routing/operation-rules"
      isLoading={(!isNew && query.isLoading) || capabilities.isLoading || channels.isLoading || profiles.isLoading || models.isLoading || rules.isLoading}
      error={query.error ?? capabilities.error ?? channels.error ?? profiles.error ?? models.error ?? rules.error}
      hasData={isNew || Boolean(rule)}
      saving={busy}
      detailCard={
        rule ? (
          <Card size="sm">
            <CardHeader>
              <CardTitle>{apiOperationLabel(rule.operation)}</CardTitle>
              <CardDescription>{rule.model_routing_profile_id}</CardDescription>
            </CardHeader>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{t("Routing graph")}</CardTitle>
            <CardDescription>
              {t("The routing profile and operation are immutable after creation.")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <FieldGroup>
              {!modelId && <Field data-invalid={Boolean(fieldError("model_routing_profile_id"))}>
                <FieldLabel htmlFor="operation-rule-profile">
                  {t("Model routing profile")}
                </FieldLabel>
                <Select
                  value={state.model_routing_profile_id}
                  items={(profiles.data ?? []).map((profile) => ({
                    value: profile.id, label: `${profile.model_display_name} (${profile.client_model})`,
                  }))}
                  disabled={!isNew}
                  onValueChange={(value) => patch({ model_routing_profile_id: value ?? "" })}
                >
                  <SelectTrigger id="operation-rule-profile" aria-invalid={Boolean(fieldError("model_routing_profile_id"))}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(profiles.data ?? []).map((profile) => (
                        <SelectItem key={profile.id} value={profile.id}>
                          {profile.model_display_name} ({profile.client_model})
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
                <FieldDescription>
                  {t("One routing profile exists per priced client model.")}
                </FieldDescription>
                {fieldError("model_routing_profile_id") ? (
                  <FieldError>{fieldError("model_routing_profile_id")}</FieldError>
                ) : null}
              </Field>}
              {isNew && !modelId ? (
                <Field>
                  <FieldLabel htmlFor="profile-model">{t("Create profile for pricing model")}</FieldLabel>
                  <Select value={newProfileModel} onValueChange={(value) => setNewProfileModel(value ?? "")}
                    items={(models.data ?? []).map((model) => ({
                      value: model.id, label: `${model.display_name} (${model.source_model_id})`,
                    }))}>
                    <SelectTrigger id="profile-model"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {(models.data ?? []).filter((model) => model.enabled &&
                          !profiles.data?.some((profile) => profile.model_id === model.id))
                          .map((model) => (
                            <SelectItem key={model.id} value={model.id}>
                              {model.display_name} ({model.source_model_id})
                            </SelectItem>
                          ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  <FieldDescription>{t("Creating a profile binds model identity without creating routes or authorization.")}</FieldDescription>
                  <Button type="button" variant="outline" disabled={!newProfileModel || busy}
                    onClick={() => void addProfile()}>
                    {t("Create routing profile")}
                  </Button>
                </Field>
              ) : null}
              <Field data-invalid={Boolean(fieldError("operation"))}>
                <FieldLabel htmlFor="operation-rule-operation">{t("Operation")}</FieldLabel>
                <Select
                  value={state.operation}
                  items={API_OPERATIONS.map((operation) => ({ value: operation, label: apiOperationLabel(operation) }))}
                  disabled={!isNew}
                  onValueChange={(value) =>
                    patch({ operation: value as ApiOperation, routing_tiers: [] })
                  }
                >
                  <SelectTrigger id="operation-rule-operation" aria-invalid={Boolean(fieldError("operation"))}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(isNew ? availableOperations : API_OPERATIONS).map((operation) => (
                        <SelectItem key={operation} value={operation}>
                          {apiOperationLabel(operation)}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
                {fieldError("operation") && <FieldError>{fieldError("operation")}</FieldError>}
              </Field>
              <OperationRuleTierEditor
                value={routingTiers}
                capabilities={operationCapabilities}
                channelNames={channelNames}
                onChange={(next) => patch({ routing_tiers: next })}
                errorFor={(path) => fieldError(path)}
              />
              <Field orientation="horizontal">
                <FieldLabel htmlFor="operation-rule-enabled">{t("Enabled")}</FieldLabel>
                <Switch
                  id="operation-rule-enabled"
                  checked={state.enabled}
                  onCheckedChange={(checked) => patch({ enabled: checked })}
                />
              </Field>
              <Button type="button" disabled={busy} onClick={() => void submit()}>
                {busy ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create operation rule") : t("Save operation rule")}
              </Button>
            </FieldGroup>
          </CardContent>
        </Card>
      }
    />
  );
}
