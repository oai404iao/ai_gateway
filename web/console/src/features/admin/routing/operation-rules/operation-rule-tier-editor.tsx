import { useId, useMemo, useState } from "react";
import { Plus, Trash2, X } from "lucide-react";
import type {
  ChannelCapabilityView,
  OperationTierInput,
  SelectionStrategy,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";
import { AllocationPath } from "./allocation-path";
import {
  Combobox,
  ComboboxCollection,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
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
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SELECTION_STRATEGIES, apiOperationLabel, selectionStrategyLabel } from "@/lib/permissions";

type RouteCandidate = OperationTierInput["candidates"][number];

interface OperationRuleTierEditorProps {
  value: OperationTierInput[];
  capabilities: ChannelCapabilityView[];
  channelNames: Map<string, string>;
  onChange: (value: OperationTierInput[]) => void;
  errorFor: (path: Array<string | number>) => string | undefined;
}

function nextPriority(tiers: OperationTierInput[]): number {
  const used = new Set(tiers.map((tier) => tier.priority));
  let priority = 0;
  while (used.has(priority)) priority += 1;
  return priority;
}

function updateAt<T>(items: T[], index: number, value: T): T[] {
  return items.map((item, itemIndex) => (itemIndex === index ? value : item));
}

function removeAt<T>(items: T[], index: number): T[] {
  return items.filter((_, itemIndex) => itemIndex !== index);
}

export function OperationRuleTierEditor({
  value,
  capabilities,
  channelNames,
  onChange,
  errorFor,
}: OperationRuleTierEditorProps) {
  const { t } = useI18n();
  const idPrefix = useId();
  const [selection, setSelection] = useState(0);
  const active = Math.min(selection, value.length - 1);
  const capabilityById = useMemo(
    () => new Map(capabilities.map((capability) => [capability.id, capability])),
    [capabilities],
  );
  const capabilityIds = useMemo(
    () =>
      [...capabilities]
        .sort((left, right) =>
          (channelNames.get(left.channel_id) ?? "").localeCompare(
            channelNames.get(right.channel_id) ?? "",
          ),
        )
        .map((capability) => capability.id),
    [capabilities, channelNames],
  );
  const capabilityLabel = (capability: ChannelCapabilityView) =>
    `${channelNames.get(capability.channel_id) ?? t("Unknown channel")} · ${apiOperationLabel(capability.settings.operation)}${capability.settings.enabled ? "" : ` · ${t("Disabled")}`}`;

  const patchTier = (tierIndex: number, partial: Partial<OperationTierInput>) => {
    onChange(updateAt(value, tierIndex, { ...value[tierIndex], ...partial }));
  };

  const patchCandidate = (
    tierIndex: number,
    candidateIndex: number,
    partial: Partial<RouteCandidate>,
  ) => {
    const tier = value[tierIndex];
    patchTier(tierIndex, {
      candidates: updateAt(tier.candidates, candidateIndex, {
        ...tier.candidates[candidateIndex],
        ...partial,
      }),
    });
  };

  return (
    <FieldSet data-invalid={Boolean(errorFor(["routing_tiers"])) || undefined}>
      <FieldLegend>{t("Operation routing tiers")}</FieldLegend>
      <FieldDescription>
        {t(
          "Lower priority tiers are tried first. Every candidate names a capability and one of its advertised upstream models.",
        )}
      </FieldDescription>
      <div className="@container min-w-0 border-y">
        <div className="grid min-w-0 @min-[42rem]:grid-cols-[minmax(0,1fr)_minmax(19rem,.9fr)]">
        <AllocationPath tiers={value} active={active} onSelect={setSelection} errorFor={errorFor}
          controls={`${idPrefix}-properties`} channelName={(id) => {
            const capability = capabilityById.get(id);
            return capability ? channelNames.get(capability.channel_id) ?? t("Unknown channel") : t(id ? "Unavailable" : "Choose a capability");
          }} />
        {value.map((tier, tierIndex) => {
          if (tierIndex !== active) return null;
          const tierPath = ["routing_tiers", tierIndex];
          const priorityError = errorFor([...tierPath, "priority"]);
          return (
            <section key={tierIndex} id={`${idPrefix}-properties`} aria-label={t("Tier properties")} className="min-w-0 border-t bg-muted/25 p-3 @min-[42rem]:border-t-0 @min-[42rem]:border-l">
              <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-sm font-medium">{t("Tier {number}", { number: tierIndex + 1 })} · {t("Tier properties")}</h3>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("Remove tier {number}", { number: tierIndex + 1 })}
                    onClick={() => onChange(removeAt(value, tierIndex))}
                  >
                    <Trash2 data-icon="inline-start" />
                  </Button>
              </div>
                <FieldGroup>
                  <FieldGroup className="grid grid-cols-[minmax(4rem,.65fr)_minmax(0,1.35fr)] gap-2">
                    <Field data-invalid={Boolean(priorityError) || undefined}>
                      <FieldLabel htmlFor={`${idPrefix}-${tierIndex}-priority`}>
                        {t("Priority")}
                      </FieldLabel>
                      <Input
                        id={`${idPrefix}-${tierIndex}-priority`}
                        type="number"
                        min={0}
                        max={2_147_483_647}
                        step={1}
                        value={tier.priority}
                        aria-invalid={Boolean(priorityError)}
                        onChange={(event) =>
                          patchTier(tierIndex, { priority: Number(event.target.value) })
                        }
                      />
                      {priorityError ? <FieldError>{priorityError}</FieldError> : null}
                    </Field>
                    <Field>
                      <FieldLabel>{t("Selection strategy")}</FieldLabel>
                      <Select
                        value={tier.selection_strategy}
                        onValueChange={(strategy) =>
                          patchTier(tierIndex, {
                            selection_strategy: strategy as SelectionStrategy,
                          })
                        }
                      >
                        <SelectTrigger
                          aria-label={t("Selection strategy for tier {number}", {
                            number: tierIndex + 1,
                          })}
                        >
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            {SELECTION_STRATEGIES.map((strategy) => (
                              <SelectItem key={strategy} value={strategy}>
                                {selectionStrategyLabel(strategy)}
                              </SelectItem>
                            ))}
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                    </Field>
                  </FieldGroup>

                  <FieldSet data-invalid={Boolean(errorFor([...tierPath, "candidates"])) || undefined}>
                    <FieldLegend className="sr-only">{t("Capability/model candidates")}</FieldLegend>
                    <ol className="@container flex flex-col gap-3">
                        {tier.candidates.map((candidate, candidateIndex) => {
                          const candidatePath = [...tierPath, "candidates", candidateIndex];
                          const capability = capabilityById.get(candidate.capability_id);
                          const capabilityError = errorFor([...candidatePath, "capability_id"]);
                          const modelError = errorFor([...candidatePath, "upstream_model"]);
                          const weightError = errorFor([...candidatePath, "weight"]);
                          const duplicate = (capabilityId: string, model: string) =>
                            tier.candidates.some(
                              (entry, index) =>
                                index !== candidateIndex &&
                                entry.capability_id === capabilityId &&
                                entry.upstream_model === model,
                            );
                          const visibleModels = [
                            ...new Set([
                              ...(capability?.settings.available_models ?? []),
                              ...(candidate.upstream_model ? [candidate.upstream_model] : []),
                            ]),
                          ];
                          const rowLabel = { tier: tierIndex + 1, row: candidateIndex + 1 };
                          const rowId = `${idPrefix}-${tierIndex}-${candidateIndex}`;
                          return (
                            <li key={candidateIndex}
                              className="grid min-w-0 grid-cols-[minmax(0,1fr)] items-start gap-2 border-t px-2 py-3 @min-[32rem]:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_5rem_auto]">
                                <Field data-invalid={Boolean(capabilityError) || undefined}>
                                  <FieldLabel htmlFor={`${rowId}-capability`}>{t("Capability")}</FieldLabel>
                                  <Combobox
                                    items={capabilityIds}
                                    value={candidate.capability_id || null}
                                    itemToStringLabel={(id) => {
                                      const option = capabilityById.get(id);
                                      return option ? capabilityLabel(option) : t("Unavailable");
                                    }}
                                    onValueChange={(capabilityId) => {
                                      if (capabilityId === null) return;
                                      const next = capabilityById.get(capabilityId);
                                      patchCandidate(tierIndex, candidateIndex, {
                                        capability_id: capabilityId,
                                        upstream_model:
                                          next?.settings.available_models.includes(
                                            candidate.upstream_model,
                                          ) &&
                                          !duplicate(capabilityId, candidate.upstream_model)
                                            ? candidate.upstream_model
                                            : "",
                                      });
                                    }}
                                  >
                                    <ComboboxInput
                                      id={`${rowId}-capability`}
                                      aria-label={t("Capability for tier {tier} row {row}", rowLabel)}
                                      aria-invalid={Boolean(capabilityError)}
                                      placeholder={t("Choose a capability")}
                                    />
                                    <ComboboxContent>
                                      <ComboboxEmpty>{t("No matching capabilities")}</ComboboxEmpty>
                                      <ComboboxList>
                                        <ComboboxGroup>
                                          <ComboboxCollection>
                                            {(capabilityId: string) => {
                                              const option = capabilityById.get(capabilityId)!;
                                              return (
                                                <ComboboxItem
                                                  key={capabilityId}
                                                  value={capabilityId}
                                                  disabled={option.settings.available_models.every(
                                                    (model) => duplicate(capabilityId, model),
                                                  )}
                                                >
                                                  <span className="truncate">{capabilityLabel(option)}</span>
                                                </ComboboxItem>
                                              );
                                            }}
                                          </ComboboxCollection>
                                        </ComboboxGroup>
                                      </ComboboxList>
                                    </ComboboxContent>
                                  </Combobox>
                                  {capabilityError ? (
                                    <FieldError>{capabilityError}</FieldError>
                                  ) : null}
                                </Field>
                                <Field data-invalid={Boolean(modelError) || undefined}>
                                  <FieldLabel htmlFor={`${rowId}-model`}>{t("Upstream model")}</FieldLabel>
                                  <Combobox
                                    items={visibleModels}
                                    value={candidate.upstream_model || null}
                                    disabled={!capability}
                                    onValueChange={(model) =>
                                      patchCandidate(tierIndex, candidateIndex, {
                                        upstream_model: model ?? "",
                                      })
                                    }
                                  >
                                    <ComboboxInput
                                      id={`${rowId}-model`}
                                      aria-label={t("Upstream model for tier {tier} row {row}", rowLabel)}
                                      aria-invalid={Boolean(modelError)}
                                      disabled={!capability}
                                      placeholder={
                                        capability
                                          ? t("Choose an upstream model")
                                          : t("Choose a capability first")
                                      }
                                    />
                                    <ComboboxContent>
                                      <ComboboxEmpty>{t("No matching models")}</ComboboxEmpty>
                                      <ComboboxList>
                                        <ComboboxGroup>
                                          <ComboboxCollection>
                                            {(model: string) => (
                                              <ComboboxItem
                                                key={model}
                                                value={model}
                                                disabled={duplicate(
                                                  candidate.capability_id,
                                                  model,
                                                )}
                                              >
                                                {model}
                                              </ComboboxItem>
                                            )}
                                          </ComboboxCollection>
                                        </ComboboxGroup>
                                      </ComboboxList>
                                    </ComboboxContent>
                                  </Combobox>
                                  {modelError ? <FieldError>{modelError}</FieldError> : null}
                                </Field>
                                <Field data-invalid={Boolean(weightError) || undefined}>
                                  <FieldLabel htmlFor={`${rowId}-weight`}>{t("Weight")}</FieldLabel>
                                  <Input
                                    id={`${rowId}-weight`}
                                    type="number"
                                    min={1}
                                    max={2_147_483_647}
                                    step={1}
                                    value={candidate.weight}
                                    aria-label={t("Weight for tier {tier} row {row}", rowLabel)}
                                    aria-invalid={Boolean(weightError)}
                                    onChange={(event) =>
                                      patchCandidate(tierIndex, candidateIndex, {
                                        weight: Number(event.target.value),
                                      })
                                    }
                                  />
                                  {weightError ? <FieldError>{weightError}</FieldError> : null}
                                </Field>
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="sm"
                                  className="justify-self-start @min-[32rem]:mt-5"
                                  aria-label={t("Remove tier {tier} row {row}", rowLabel)}
                                  onClick={() =>
                                    patchTier(tierIndex, {
                                      candidates: removeAt(tier.candidates, candidateIndex),
                                    })
                                  }
                                >
                                  <X data-icon="inline-start" />
                                  <span className="@min-[32rem]:sr-only">{t("Remove")}</span>
                                </Button>
                            </li>
                          );
                        })}
                    </ol>
                    {tier.candidates.length === 0 ? (
                      <FieldDescription>
                        {t("Add at least one capability/model candidate.")}
                      </FieldDescription>
                    ) : null}
                    {errorFor([...tierPath, "candidates"]) ? (
                      <FieldError>{errorFor([...tierPath, "candidates"])}</FieldError>
                    ) : null}
                    <Button
                      type="button"
                      variant="outline"
                      className="self-start"
                      onClick={() =>
                        patchTier(tierIndex, {
                          candidates: [
                            ...tier.candidates,
                            { capability_id: "", upstream_model: "", weight: 1 },
                          ],
                        })
                      }
                    >
                      <Plus data-icon="inline-start" />
                      {t("Add record")}
                    </Button>
                  </FieldSet>
                </FieldGroup>
            </section>
          );
        })}
        </div>
      </div>
      {errorFor(["routing_tiers"]) ? (
        <FieldError>{errorFor(["routing_tiers"])}</FieldError>
      ) : null}
      <Button
        type="button"
        variant="outline"
        className="self-start"
        onClick={() => {
          setSelection(value.length);
          onChange([
            ...value,
            {
              priority: nextPriority(value),
              selection_strategy: "weighted_random",
              candidates: [],
            },
          ]);
        }}
      >
        <Plus data-icon="inline-start" />
        {t("Add routing tier")}
      </Button>
    </FieldSet>
  );
}
