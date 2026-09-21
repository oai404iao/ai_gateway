import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
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
  useUpdateOperationRule,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { API_OPERATIONS, apiOperationLabel } from "@/lib/permissions";
import type { ApiOperation, OperationRuleInput, OperationTierInput } from "@/api/types";
import { OperationRuleTierEditor } from "./operation-rule-tier-editor";

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
      "chat_completions",
      "responses",
      "standalone_web_search",
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

export function OperationRuleDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { t } = useI18n();
  const query = useOperationRule(id);
  const capabilities = useChannelCapabilities();
  const channels = useLogicalChannels();
  const create = useCreateOperationRule();
  const update = useUpdateOperationRule(id);
  const [state, setState] = useState<FormState>(empty);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const rule = query.data?.data;
  const busy = submitting || create.isPending || update.isPending;

  useEffect(() => {
    if (!rule) return;
    setState({
      model_routing_profile_id: rule.model_routing_profile_id,
      operation: rule.operation,
      enabled: rule.enabled,
      routing_tiers: rule.routing_tiers,
    });
  }, [rule]);

  const channelNames = useMemo(
    () => new Map((channels.data ?? []).map((channel) => [channel.id, channel.name])),
    [channels.data],
  );
  const operationCapabilities = useMemo(
    () => (capabilities.data ?? []).filter((capability) => capability.settings.operation === state.operation),
    [capabilities.data, state.operation],
  );

  const patch = (partial: Partial<FormState>) => {
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
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    try {
      const input = parsed.data satisfies OperationRuleInput;
      if (isNew) {
        const result = await create.mutateAsync(input);
        toast.success(t("Operation rule created"));
        navigate(`/admin/routing/operation-rules/${result.id}`, { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
        toast.success(t("Operation rule saved"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This rule was changed elsewhere. Reloading."));
        await query.refetch();
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
      title={isNew ? t("New operation rule") : t("Operation rule")}
      description={t(
        "Candidates reference an explicit capability and upstream model. Tiers, priorities, and weights are never inferred from legacy channels.",
      )}
      backPath="/admin/routing/operation-rules"
      isLoading={!isNew && (query.isLoading || capabilities.isLoading || channels.isLoading)}
      error={query.error ?? capabilities.error ?? channels.error}
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
              <Field data-invalid={Boolean(fieldError("model_routing_profile_id"))}>
                <FieldLabel htmlFor="operation-rule-profile">
                  {t("Model routing profile")}
                </FieldLabel>
                <Input
                  id="operation-rule-profile"
                  value={state.model_routing_profile_id}
                  disabled={!isNew}
                  aria-invalid={Boolean(fieldError("model_routing_profile_id"))}
                  onChange={(event) =>
                    patch({ model_routing_profile_id: event.target.value })
                  }
                />
                <FieldDescription>
                  {t("One routing profile exists per priced client model.")}
                </FieldDescription>
                {fieldError("model_routing_profile_id") ? (
                  <FieldError>{fieldError("model_routing_profile_id")}</FieldError>
                ) : null}
              </Field>
              <Field>
                <FieldLabel htmlFor="operation-rule-operation">{t("Operation")}</FieldLabel>
                <Select
                  value={state.operation}
                  disabled={!isNew}
                  onValueChange={(value) =>
                    patch({ operation: value as ApiOperation, routing_tiers: [] })
                  }
                >
                  <SelectTrigger id="operation-rule-operation">
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
