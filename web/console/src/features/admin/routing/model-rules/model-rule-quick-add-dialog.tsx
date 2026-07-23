import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useChannelGroups,
  useChannels,
  useCreateModelRule,
  useModelRules,
  useModels,
} from "@/features/admin/api";
import { groupModelsByProvider } from "@/features/admin/models/model-groups";
import {
  buildQuickAddModelPlans,
  type QuickAddFormatPlan,
  type QuickAddModelPlan,
} from "@/features/admin/routing/model-rules/model-rule-quick-add-plan";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

interface ModelRuleQuickAddDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function formatBadgeVariant(
  status: QuickAddFormatPlan["status"],
): "info" | "secondary" | "warning" {
  if (status === "ready") return "info";
  if (status === "strategy_conflict") return "warning";
  return "secondary";
}

export function ModelRuleQuickAddDialog({
  open,
  onOpenChange,
}: ModelRuleQuickAddDialogProps) {
  const models = useModels();
  const groups = useChannelGroups();
  const channels = useChannels();
  const rules = useModelRules();
  const create = useCreateModelRule();
  const { t } = useI18n();
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);

  const plans = useMemo(
    () =>
      buildQuickAddModelPlans(
        models.data ?? [],
        groups.data ?? [],
        channels.data ?? [],
        rules.data ?? [],
      ),
    [channels.data, groups.data, models.data, rules.data],
  );
  const planByModelId = useMemo(
    () => new Map(plans.map((plan) => [plan.model.id, plan])),
    [plans],
  );
  const selectableIds = useMemo(
    () => new Set(plans.filter((plan) => plan.drafts.length > 0).map((plan) => plan.model.id)),
    [plans],
  );

  useEffect(() => {
    if (!open) {
      setSearch("");
      setSelected(new Set());
      return;
    }
    setSelected((current) => {
      const next = new Set([...current].filter((id) => selectableIds.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [open, selectableIds]);

  const normalizedSearch = search.trim().toLocaleLowerCase();
  const filteredPlans = useMemo(
    () =>
      plans.filter((plan) => {
        if (!normalizedSearch) return true;
        return [
          plan.model.display_name,
          plan.model.source_model_id,
          plan.model.provider_name ?? "",
        ].some((value) => value.toLocaleLowerCase().includes(normalizedSearch));
      }),
    [normalizedSearch, plans],
  );
  const providerGroups = useMemo(
    () =>
      groupModelsByProvider(
        filteredPlans.map((plan) => plan.model),
        t("Unspecified provider"),
      ),
    [filteredPlans, t],
  );
  const selectedPlans = plans.filter((plan) => selected.has(plan.model.id));
  const selectedDrafts = selectedPlans.flatMap((plan) => plan.drafts);
  const isLoading =
    models.isLoading || groups.isLoading || channels.isLoading || rules.isLoading;
  const error = models.error ?? groups.error ?? channels.error ?? rules.error;

  const toggle = (plan: QuickAddModelPlan) => {
    if (plan.drafts.length === 0) return;
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(plan.model.id)) next.delete(plan.model.id);
      else next.add(plan.model.id);
      return next;
    });
  };

  const selectAllAvailable = () => setSelected(new Set(selectableIds));

  const unavailableDescription = (plan: QuickAddModelPlan): string | undefined => {
    if (!plan.model.enabled) return t("Upstream model is disabled.");
    if (plan.formats.some((format) => format.status === "strategy_conflict")) {
      return t("Routing groups at the same priority use different selection strategies.");
    }
    if (plan.drafts.length > 0) return undefined;
    if (plan.formats.some((format) => format.status === "configured")) {
      return t("All compatible formats already have rules.");
    }
    if (plan.drafts.length === 0) return t("No compatible enabled channels.");
    return undefined;
  };

  const submit = async () => {
    if (selectedDrafts.length === 0) return;
    setSubmitting(true);
    let created = 0;
    const failures: unknown[] = [];
    for (const input of selectedDrafts) {
      try {
        await create.mutateAsync(input);
        created += 1;
      } catch (error) {
        failures.push(error);
      }
    }

    if (failures.length === 0) {
      toast.success(t("Created {count} model rules.", { count: created }));
      onOpenChange(false);
    } else if (created > 0) {
      toast.error(
        t("Created {created} of {total} model rules. Refresh and retry the remaining items.", {
          created,
          total: selectedDrafts.length,
        }),
      );
      onOpenChange(false);
    } else {
      toast.error(
        controlPlaneMutationErrorMessage(failures[0], t("Batch create failed")),
      );
    }
    setSubmitting(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!submitting) onOpenChange(nextOpen);
      }}
    >
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("Quick add model rules")}</DialogTitle>
          <DialogDescription>
            {t(
              "Select upstream models. Missing rules are created for every API format with compatible enabled channels. Each rule uses the source model id, stays enabled, and targets every compatible channel; complete channel groups are selected when possible.",
            )}
          </DialogDescription>
        </DialogHeader>

        <AsyncResource
          isLoading={isLoading}
          error={error}
          isEmpty={(models.data?.length ?? 0) === 0}
          emptyTitle={t("No upstream models")}
          emptyDescription={t("Create or import upstream models before adding routing rules.")}
        >
          <div className="flex flex-col gap-4">
            <Field>
              <FieldLabel htmlFor="quick-add-model-search">{t("Search models")}</FieldLabel>
              <Input
                id="quick-add-model-search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder={t("Search by model id, name, or provider")}
                disabled={submitting}
              />
            </Field>

            <div className="flex flex-wrap items-center justify-between gap-2">
              <FieldDescription>
                {t("Selected {models} models; {rules} rules will be created.", {
                  models: selectedPlans.length,
                  rules: selectedDrafts.length,
                })}
              </FieldDescription>
              <div className="flex items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={selectAllAvailable}
                  disabled={selectableIds.size === 0 || submitting}
                >
                  {t("Select all available")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelected(new Set())}
                  disabled={selected.size === 0 || submitting}
                >
                  {t("Clear selection")}
                </Button>
              </div>
            </div>

            <ScrollArea className="h-[min(50svh,28rem)] rounded-md border">
              {providerGroups.length > 0 ? (
                <div className="flex flex-col gap-5 p-3">
                  {providerGroups.map((providerGroup) => (
                    <FieldSet key={providerGroup.provider}>
                      <FieldLegend variant="label">{providerGroup.provider}</FieldLegend>
                      <FieldGroup data-slot="checkbox-group" className="gap-3">
                        {providerGroup.models.map((model) => {
                          const plan = planByModelId.get(model.id);
                          if (!plan) return null;
                          const disabled = plan.drafts.length === 0;
                          const description = unavailableDescription(plan);
                          return (
                            <Field
                              key={model.id}
                              orientation="horizontal"
                              data-disabled={disabled}
                            >
                              <Checkbox
                                id={`quick_add_model_${model.id}`}
                                checked={selected.has(model.id)}
                                disabled={disabled || submitting}
                                aria-label={`${t("Select")} ${model.source_model_id}`}
                                onCheckedChange={() => toggle(plan)}
                              />
                              <FieldContent>
                                <FieldLabel htmlFor={`quick_add_model_${model.id}`}>
                                  {model.display_name}
                                </FieldLabel>
                                <FieldDescription className="font-mono text-xs">
                                  {model.source_model_id}
                                </FieldDescription>
                                <div className="flex flex-wrap gap-1">
                                  {plan.formats
                                    .filter((format) => format.status !== "no_channels")
                                    .map((format) => (
                                      <Badge
                                        key={format.apiFormat}
                                        variant={formatBadgeVariant(format.status)}
                                      >
                                        {apiFormatLabel(format.apiFormat)}
                                        {" · "}
                                        {format.status === "ready"
                                          ? t("{count} compatible channels", {
                                              count: format.compatibleChannelCount,
                                            })
                                          : format.status === "configured"
                                            ? t("Already configured")
                                            : format.status === "strategy_conflict"
                                              ? t("Manual setup required")
                                              : t("Model disabled")}
                                      </Badge>
                                    ))}
                                </div>
                                {description ? (
                                  <FieldDescription>{description}</FieldDescription>
                                ) : null}
                              </FieldContent>
                            </Field>
                          );
                        })}
                      </FieldGroup>
                    </FieldSet>
                  ))}
                </div>
              ) : (
                <EmptyState
                  title={t("No models match this search.")}
                  description={t("Try a different model id, name, or provider.")}
                  className="min-h-48"
                />
              )}
            </ScrollArea>
          </div>
        </AsyncResource>

        <DialogFooter>
          <DialogClose render={<Button variant="outline" disabled={submitting} />}>
            {t("Cancel")}
          </DialogClose>
          <Button
            onClick={submit}
            disabled={submitting || isLoading || Boolean(error) || selectedDrafts.length === 0}
          >
            {submitting ? <Spinner data-icon="inline-start" /> : null}
            {t("Create {count} rules", { count: selectedDrafts.length })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
