import { useId, useMemo } from "react";
import { Plus, Trash2, X } from "lucide-react";
import type {
  ChannelGroupView,
  ChannelView,
  ModelProtocolRuleInput,
  SelectionStrategy,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  SELECTION_STRATEGIES,
  selectionStrategyLabel,
} from "@/lib/permissions";
import { cn } from "@/lib/utils";

type RoutingTier = ModelProtocolRuleInput["routing_tiers"][number];
type RouteCandidate = RoutingTier["candidates"][number];

interface ModelRuleTierEditorProps {
  value: RoutingTier[];
  groups: ChannelGroupView[];
  channels: ChannelView[];
  onChange: (value: RoutingTier[]) => void;
  errorFor: (path: Array<string | number>) => string | undefined;
  className?: string;
}

function nextPriority(tiers: RoutingTier[]): number {
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

export function ModelRuleTierEditor({
  value,
  groups,
  channels,
  onChange,
  errorFor,
  className,
}: ModelRuleTierEditorProps) {
  const { t } = useI18n();
  const idPrefix = useId();
  const groupById = useMemo(
    () => new Map(groups.map((group) => [group.id, group])),
    [groups],
  );
  const channelById = useMemo(
    () => new Map(channels.map((channel) => [channel.id, channel])),
    [channels],
  );
  const channelIds = useMemo(
    () =>
      [...channels]
        .sort((left, right) => left.name.localeCompare(right.name))
        .map((channel) => channel.id),
    [channels],
  );

  const patchTier = (tierIndex: number, partial: Partial<RoutingTier>) => {
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
    <FieldSet
      className={cn(className)}
      data-invalid={Boolean(errorFor(["routing_tiers"])) || undefined}
    >
      <FieldLegend>{t("Protocol routing tiers")}</FieldLegend>
      <FieldDescription>
        {t(
          "Lower priority tiers are tried first. Weights are compared only among eligible candidates in the same tier.",
        )}
      </FieldDescription>
      <FieldGroup>
        {value.map((tier, tierIndex) => {
          const tierPath = ["routing_tiers", tierIndex];
          const priorityError = errorFor([...tierPath, "priority"]);
          return (
            <Card key={tierIndex}>
              <CardHeader>
                <CardTitle>
                  {t("Tier {number}", { number: tierIndex + 1 })}
                </CardTitle>
                <CardDescription>
                  {t("{count} route candidates", {
                    count: tier.candidates.length,
                  })}
                </CardDescription>
                <CardAction>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("Remove tier {number}", {
                      number: tierIndex + 1,
                    })}
                    onClick={() => onChange(removeAt(value, tierIndex))}
                  >
                    <Trash2 data-icon="inline-start" />
                  </Button>
                </CardAction>
              </CardHeader>
              <CardContent>
                <FieldGroup>
                  <FieldGroup className="grid gap-4 sm:grid-cols-[8rem_14rem]">
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
                          patchTier(tierIndex, {
                            priority: Number(event.target.value),
                          })
                        }
                      />
                      {priorityError ? (
                        <FieldError>{priorityError}</FieldError>
                      ) : null}
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
                          aria-label={t(
                            "Selection strategy for tier {number}",
                            { number: tierIndex + 1 },
                          )}
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

                  <FieldSet
                    data-invalid={
                      Boolean(errorFor([...tierPath, "candidates"])) ||
                      undefined
                    }
                  >
                    <FieldLegend className="sr-only">
                      {t("Channel/model candidates")}
                    </FieldLegend>
                    <Table className="min-w-[34rem] table-fixed">
                      <TableHeader>
                        <TableRow>
                          <TableHead className="w-[38%]">
                            {t("Channel")}
                          </TableHead>
                          <TableHead>{t("Upstream model")}</TableHead>
                          <TableHead className="w-24">{t("Weight")}</TableHead>
                          <TableHead className="w-10">
                            <span className="sr-only">{t("Actions")}</span>
                          </TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {tier.candidates.map((candidate, candidateIndex) => {
                          const candidatePath = [
                            ...tierPath,
                            "candidates",
                            candidateIndex,
                          ];
                          const channel = channelById.get(candidate.channel_id);
                          const channelError = errorFor([
                            ...candidatePath,
                            "channel_id",
                          ]);
                          const modelError = errorFor([
                            ...candidatePath,
                            "upstream_model",
                          ]);
                          const weightError = errorFor([
                            ...candidatePath,
                            "weight",
                          ]);
                          const duplicate = (
                            channelId: string,
                            model: string,
                          ) =>
                            tier.candidates.some(
                              (entry, index) =>
                                index !== candidateIndex &&
                                entry.channel_id === channelId &&
                                entry.upstream_model === model,
                            );
                          const visibleModels = [
                            ...new Set([
                              ...(channel?.available_models ?? []),
                              ...(candidate.upstream_model
                                ? [candidate.upstream_model]
                                : []),
                            ]),
                          ];
                          const rowLabel = {
                            tier: tierIndex + 1,
                            row: candidateIndex + 1,
                          };
                          return (
                            <TableRow key={candidateIndex}>
                              <TableCell className="align-top whitespace-normal">
                                <Field
                                  data-invalid={
                                    Boolean(channelError) || undefined
                                  }
                                >
                                  <Combobox
                                    items={channelIds}
                                    value={candidate.channel_id || null}
                                    itemToStringLabel={(id) =>
                                      channelById.get(id)?.name ?? id
                                    }
                                    onValueChange={(channelId) => {
                                      // Clearing the search must not clear the dependent model.
                                      if (channelId === null) return;
                                      const nextChannel =
                                        channelById.get(channelId);
                                      patchCandidate(
                                        tierIndex,
                                        candidateIndex,
                                        {
                                          channel_id: channelId,
                                          upstream_model:
                                            nextChannel?.available_models.includes(
                                              candidate.upstream_model,
                                            ) &&
                                            !duplicate(
                                              nextChannel.id,
                                              candidate.upstream_model,
                                            )
                                              ? candidate.upstream_model
                                              : "",
                                        },
                                      );
                                    }}
                                  >
                                    <ComboboxInput
                                      aria-label={t(
                                        "Channel for tier {tier} row {row}",
                                        rowLabel,
                                      )}
                                      aria-invalid={Boolean(channelError)}
                                      placeholder={t("Choose a channel")}
                                    />
                                    <ComboboxContent>
                                      <ComboboxEmpty>
                                        {t("No matching channels")}
                                      </ComboboxEmpty>
                                      <ComboboxList>
                                        <ComboboxGroup>
                                          <ComboboxCollection>
                                            {(channelId: string) => {
                                              const option =
                                                channelById.get(channelId)!;
                                              return (
                                                <ComboboxItem
                                                  key={channelId}
                                                  value={channelId}
                                                  disabled={
                                                    !option.available_models.some(
                                                      (model) =>
                                                        !duplicate(
                                                          channelId,
                                                          model,
                                                        ),
                                                    )
                                                  }
                                                >
                                                  <span className="flex min-w-0 flex-col">
                                                    <span className="truncate">
                                                      {option.name}
                                                    </span>
                                                    <span className="truncate text-xs text-muted-foreground">
                                                      {
                                                        groupById.get(
                                                          option.channel_group_id,
                                                        )?.name
                                                      }
                                                    </span>
                                                  </span>
                                                </ComboboxItem>
                                              );
                                            }}
                                          </ComboboxCollection>
                                        </ComboboxGroup>
                                      </ComboboxList>
                                    </ComboboxContent>
                                  </Combobox>
                                  {channelError ? (
                                    <FieldError>{channelError}</FieldError>
                                  ) : null}
                                </Field>
                              </TableCell>
                              <TableCell className="align-top whitespace-normal">
                                <Field
                                  data-invalid={
                                    Boolean(modelError) || undefined
                                  }
                                >
                                  <Combobox
                                    items={visibleModels}
                                    value={candidate.upstream_model || null}
                                    disabled={!channel}
                                    onValueChange={(model) =>
                                      patchCandidate(
                                        tierIndex,
                                        candidateIndex,
                                        { upstream_model: model ?? "" },
                                      )
                                    }
                                  >
                                    <ComboboxInput
                                      aria-label={t(
                                        "Upstream model for tier {tier} row {row}",
                                        rowLabel,
                                      )}
                                      aria-invalid={Boolean(modelError)}
                                      disabled={!channel}
                                      placeholder={
                                        channel
                                          ? t("Choose an upstream model")
                                          : t("Choose a channel first")
                                      }
                                    />
                                    <ComboboxContent>
                                      <ComboboxEmpty>
                                        {t("No matching models")}
                                      </ComboboxEmpty>
                                      <ComboboxList>
                                        <ComboboxGroup>
                                          <ComboboxCollection>
                                            {(model: string) => (
                                              <ComboboxItem
                                                key={model}
                                                value={model}
                                                disabled={duplicate(
                                                  candidate.channel_id,
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
                                  {modelError ? (
                                    <FieldError>{modelError}</FieldError>
                                  ) : null}
                                </Field>
                              </TableCell>
                              <TableCell className="align-top whitespace-normal">
                                <Field
                                  data-invalid={
                                    Boolean(weightError) || undefined
                                  }
                                >
                                  <Input
                                    type="number"
                                    min={1}
                                    max={2_147_483_647}
                                    step={1}
                                    value={candidate.weight}
                                    aria-label={t(
                                      "Weight for tier {tier} row {row}",
                                      rowLabel,
                                    )}
                                    aria-invalid={Boolean(weightError)}
                                    onChange={(event) =>
                                      patchCandidate(
                                        tierIndex,
                                        candidateIndex,
                                        { weight: Number(event.target.value) },
                                      )
                                    }
                                  />
                                  {weightError ? (
                                    <FieldError>{weightError}</FieldError>
                                  ) : null}
                                </Field>
                              </TableCell>
                              <TableCell className="align-top">
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="icon-sm"
                                  aria-label={t(
                                    "Remove tier {tier} row {row}",
                                    rowLabel,
                                  )}
                                  onClick={() =>
                                    patchTier(tierIndex, {
                                      candidates: removeAt(
                                        tier.candidates,
                                        candidateIndex,
                                      ),
                                    })
                                  }
                                >
                                  <X data-icon="inline-start" />
                                </Button>
                              </TableCell>
                            </TableRow>
                          );
                        })}
                      </TableBody>
                    </Table>
                    {tier.candidates.length === 0 ? (
                      <FieldDescription>
                        {t("Add at least one channel/model candidate.")}
                      </FieldDescription>
                    ) : null}
                    {errorFor([...tierPath, "candidates"]) ? (
                      <FieldError>
                        {errorFor([...tierPath, "candidates"])}
                      </FieldError>
                    ) : null}
                    <Button
                      type="button"
                      variant="outline"
                      className="self-start"
                      onClick={() =>
                        patchTier(tierIndex, {
                          candidates: [
                            ...tier.candidates,
                            { channel_id: "", upstream_model: "", weight: 1 },
                          ],
                        })
                      }
                    >
                      <Plus data-icon="inline-start" />
                      {t("Add record")}
                    </Button>
                  </FieldSet>
                </FieldGroup>
              </CardContent>
            </Card>
          );
        })}
      </FieldGroup>
      {errorFor(["routing_tiers"]) ? (
        <FieldError>{errorFor(["routing_tiers"])}</FieldError>
      ) : null}
      <Button
        type="button"
        variant="outline"
        className="self-start"
        onClick={() =>
          onChange([
            ...value,
            {
              priority: nextPriority(value),
              selection_strategy: "weighted_random",
              candidates: [],
            },
          ])
        }
      >
        <Plus data-icon="inline-start" />
        {t("Add routing tier")}
      </Button>
    </FieldSet>
  );
}
