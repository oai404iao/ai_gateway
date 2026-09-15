import { useId, useMemo } from "react";
import { Plus, Trash2 } from "lucide-react";
import type {
  ChannelGroupView,
  ChannelView,
  ModelProtocolRuleInput,
  SelectionStrategy,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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

const ADD_CHANNEL_VALUE = "__add_channel__";
const ADD_GROUP_VALUE = "__add_channel_group__";

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

function candidateKey(candidate: RouteCandidate): string {
  return `${candidate.channel_id}\u0000${candidate.upstream_model}`;
}

function nextUnusedModel(
  candidates: readonly RouteCandidate[],
  channel: ChannelView,
): string | undefined {
  const selected = new Set(
    candidates
      .filter((candidate) => candidate.channel_id === channel.id)
      .map((candidate) => candidate.upstream_model),
  );
  return channel.available_models.find((model) => !selected.has(model));
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

  const addChannel = (tierIndex: number, channelId: string) => {
    const tier = value[tierIndex];
    const channel = channelById.get(channelId);
    if (!channel) return;
    const upstreamModel = nextUnusedModel(tier.candidates, channel);
    if (!upstreamModel) return;
    patchTier(tierIndex, {
      candidates: [
        ...tier.candidates,
        {
          channel_id: channel.id,
          upstream_model: upstreamModel,
          weight: 100,
        },
      ],
    });
  };

  const addChannelGroup = (tierIndex: number, groupId: string) => {
    const tier = value[tierIndex];
    const selectedChannelIds = new Set(
      tier.candidates.map((candidate) => candidate.channel_id),
    );
    const additions = channels
      .filter(
        (channel) =>
          channel.channel_group_id === groupId &&
          !selectedChannelIds.has(channel.id),
      )
      .sort((left, right) => left.name.localeCompare(right.name))
      .flatMap((channel) => {
        const upstreamModel = channel.available_models[0];
        return upstreamModel
          ? [
              {
                channel_id: channel.id,
                upstream_model: upstreamModel,
                weight: 100,
              },
            ]
          : [];
      });
    if (additions.length === 0) return;
    patchTier(tierIndex, {
      candidates: [...tier.candidates, ...additions],
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
          "Lower priority tiers are tried first. Every channel and upstream-model pair has its own weight.",
        )}
      </FieldDescription>

      <FieldGroup>
        {value.map((tier, tierIndex) => {
          const tierPath = ["routing_tiers", tierIndex];
          const availableChannels = channels
            .filter((channel) => nextUnusedModel(tier.candidates, channel))
            .sort((left, right) => left.name.localeCompare(right.name));
          const availableGroups = groups
            .filter((group) =>
              channels.some(
                (channel) =>
                  channel.channel_group_id === group.id &&
                  channel.available_models.length > 0 &&
                  !tier.candidates.some(
                    (candidate) => candidate.channel_id === channel.id,
                  ),
              ),
            )
            .sort((left, right) => left.name.localeCompare(right.name));

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
                  <FieldGroup className="grid gap-4 md:grid-cols-2">
                    <Field
                      data-invalid={
                        Boolean(errorFor([...tierPath, "priority"])) || undefined
                      }
                    >
                      <FieldLabel
                        htmlFor={`${idPrefix}-tier-${tierIndex}-priority`}
                      >
                        {t("Priority")}
                      </FieldLabel>
                      <Input
                        id={`${idPrefix}-tier-${tierIndex}-priority`}
                        type="number"
                        min={0}
                        max={2_147_483_647}
                        step={1}
                        value={tier.priority}
                        aria-invalid={Boolean(
                          errorFor([...tierPath, "priority"]),
                        )}
                        onChange={(event) =>
                          patchTier(tierIndex, {
                            priority: Number(event.target.value),
                          })
                        }
                      />
                      <FieldDescription>
                        {t("Lower numbers are attempted first.")}
                      </FieldDescription>
                      {errorFor([...tierPath, "priority"]) ? (
                        <FieldError>
                          {errorFor([...tierPath, "priority"])}
                        </FieldError>
                      ) : null}
                    </Field>
                    <Field
                      data-invalid={
                        Boolean(
                          errorFor([...tierPath, "selection_strategy"]),
                        ) || undefined
                      }
                    >
                      <FieldLabel>{t("Selection strategy")}</FieldLabel>
                      <Select
                        value={tier.selection_strategy}
                        onValueChange={(selectionStrategy) =>
                          patchTier(tierIndex, {
                            selection_strategy:
                              selectionStrategy as SelectionStrategy,
                          })
                        }
                      >
                        <SelectTrigger
                          aria-label={t("Selection strategy for tier {number}", {
                            number: tierIndex + 1,
                          })}
                          aria-invalid={Boolean(
                            errorFor([...tierPath, "selection_strategy"]),
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
                      <FieldDescription>
                        {t(
                          "Weights are compared only among eligible candidates in this tier.",
                        )}
                      </FieldDescription>
                    </Field>
                  </FieldGroup>

                  <FieldGroup className="grid gap-4 md:grid-cols-2">
                    <Field>
                      <FieldLabel>{t("Add a channel")}</FieldLabel>
                      <Select
                        value={ADD_CHANNEL_VALUE}
                        disabled={availableChannels.length === 0}
                        onValueChange={(channelId) => {
                          if (channelId !== ADD_CHANNEL_VALUE) {
                            addChannel(tierIndex, channelId);
                          }
                        }}
                      >
                        <SelectTrigger
                          aria-label={t("Add channel to tier {number}", {
                            number: tierIndex + 1,
                          })}
                        >
                          <SelectValue
                            placeholder={
                              availableChannels.length > 0
                                ? t("Choose a channel")
                                : t("All channel/model pairs are already used")
                            }
                          />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem value={ADD_CHANNEL_VALUE}>
                              {t("Choose a channel")}
                            </SelectItem>
                            {availableChannels.map((channel) => (
                              <SelectItem key={channel.id} value={channel.id}>
                                {channel.name}
                              </SelectItem>
                            ))}
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                      <FieldDescription>
                        {t(
                          "Selecting the same channel again adds its next unused upstream model.",
                        )}
                      </FieldDescription>
                    </Field>

                    <Field>
                      <FieldLabel>{t("Bulk-add a channel group")}</FieldLabel>
                      <Select
                        value={ADD_GROUP_VALUE}
                        disabled={availableGroups.length === 0}
                        onValueChange={(groupId) => {
                          if (groupId !== ADD_GROUP_VALUE) {
                            addChannelGroup(tierIndex, groupId);
                          }
                        }}
                      >
                        <SelectTrigger
                          aria-label={t(
                            "Bulk-add channel group to tier {number}",
                            { number: tierIndex + 1 },
                          )}
                        >
                          <SelectValue
                            placeholder={
                              availableGroups.length > 0
                                ? t("Choose a channel group")
                                : t("No unused group channels")
                            }
                          />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem value={ADD_GROUP_VALUE}>
                              {t("Choose a channel group")}
                            </SelectItem>
                            {availableGroups.map((group) => (
                              <SelectItem key={group.id} value={group.id}>
                                {group.name}
                              </SelectItem>
                            ))}
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                      <FieldDescription>
                        {t(
                          "This expands current group members into explicit candidates. Future group changes do not alter the saved route.",
                        )}
                      </FieldDescription>
                    </Field>
                  </FieldGroup>

                  <FieldSet
                    data-invalid={
                      Boolean(errorFor([...tierPath, "candidates"])) ||
                      undefined
                    }
                  >
                    <FieldLegend variant="label">
                      {t("Channel/model candidates")}
                    </FieldLegend>
                    <FieldDescription>
                      {t(
                        "A channel may appear more than once when each entry uses a different upstream model.",
                      )}
                    </FieldDescription>
                    {tier.candidates.length > 0 ? (
                      <FieldGroup>
                        {tier.candidates.map((candidate, candidateIndex) => {
                          const candidatePath = [
                            ...tierPath,
                            "candidates",
                            candidateIndex,
                          ];
                          const channel = channelById.get(candidate.channel_id);
                          const group = channel
                            ? groupById.get(channel.channel_group_id)
                            : undefined;
                          const visibleModels =
                            channel &&
                            !channel.available_models.includes(
                              candidate.upstream_model,
                            )
                              ? [
                                  candidate.upstream_model,
                                  ...channel.available_models,
                                ]
                              : (channel?.available_models ?? [
                                  candidate.upstream_model,
                                ]);
                          const pairError =
                            errorFor([...candidatePath, "channel_id"]) ??
                            errorFor([...candidatePath, "upstream_model"]);
                          const weightError = errorFor([
                            ...candidatePath,
                            "weight",
                          ]);
                          return (
                            <Card
                              key={`${candidateKey(candidate)}-${candidateIndex}`}
                              size="sm"
                            >
                              <CardHeader>
                                <CardTitle>
                                  {channel?.name ?? candidate.channel_id}
                                </CardTitle>
                                <CardDescription>
                                  {group?.name ??
                                    channel?.channel_group_id ??
                                    t("Unknown channel")}
                                </CardDescription>
                                <CardAction>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon-sm"
                                    aria-label={t(
                                      "Remove route candidate {name}",
                                      {
                                        name:
                                          channel?.name ??
                                          candidate.channel_id,
                                      },
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
                                    <Trash2 data-icon="inline-start" />
                                  </Button>
                                </CardAction>
                              </CardHeader>
                              <CardContent>
                                <FieldGroup className="grid gap-4 md:grid-cols-[minmax(0,1fr)_8rem]">
                                  <Field
                                    data-invalid={
                                      Boolean(pairError) || undefined
                                    }
                                  >
                                    <FieldLabel>{t("Upstream model")}</FieldLabel>
                                    <Select
                                      value={candidate.upstream_model}
                                      onValueChange={(upstreamModel) =>
                                        patchCandidate(
                                          tierIndex,
                                          candidateIndex,
                                          {
                                            upstream_model: upstreamModel,
                                          },
                                        )
                                      }
                                    >
                                      <SelectTrigger
                                        aria-label={t(
                                          "Upstream model for channel {name}",
                                          {
                                            name:
                                              channel?.name ??
                                              candidate.channel_id,
                                          },
                                        )}
                                        aria-invalid={Boolean(pairError)}
                                      >
                                        <SelectValue />
                                      </SelectTrigger>
                                      <SelectContent>
                                        <SelectGroup>
                                          {visibleModels.map((model) => {
                                            const duplicate =
                                              model !==
                                                candidate.upstream_model &&
                                              tier.candidates.some(
                                                (entry, index) =>
                                                  index !== candidateIndex &&
                                                  entry.channel_id ===
                                                    candidate.channel_id &&
                                                  entry.upstream_model ===
                                                    model,
                                              );
                                            return (
                                              <SelectItem
                                                key={model}
                                                value={model}
                                                disabled={duplicate}
                                              >
                                                {model}
                                              </SelectItem>
                                            );
                                          })}
                                        </SelectGroup>
                                      </SelectContent>
                                    </Select>
                                    {pairError ? (
                                      <FieldError>{pairError}</FieldError>
                                    ) : null}
                                  </Field>
                                  <Field
                                    data-invalid={
                                      Boolean(weightError) || undefined
                                    }
                                  >
                                    <FieldLabel
                                      htmlFor={`${idPrefix}-tier-${tierIndex}-candidate-${candidateIndex}-weight`}
                                    >
                                      {t("Weight")}
                                    </FieldLabel>
                                    <Input
                                      id={`${idPrefix}-tier-${tierIndex}-candidate-${candidateIndex}-weight`}
                                      type="number"
                                      min={1}
                                      max={2_147_483_647}
                                      step={1}
                                      value={candidate.weight}
                                      aria-invalid={Boolean(weightError)}
                                      onChange={(event) =>
                                        patchCandidate(
                                          tierIndex,
                                          candidateIndex,
                                          {
                                            weight: Number(event.target.value),
                                          },
                                        )
                                      }
                                    />
                                    {weightError ? (
                                      <FieldError>{weightError}</FieldError>
                                    ) : null}
                                  </Field>
                                </FieldGroup>
                              </CardContent>
                              <CardFooter>
                                <Badge variant="outline">
                                  {t("Explicit candidate")}
                                </Badge>
                              </CardFooter>
                            </Card>
                          );
                        })}
                      </FieldGroup>
                    ) : (
                      <FieldDescription>
                        {t("Add at least one channel/model candidate.")}
                      </FieldDescription>
                    )}
                    {errorFor([...tierPath, "candidates"]) ? (
                      <FieldError>
                        {errorFor([...tierPath, "candidates"])}
                      </FieldError>
                    ) : null}
                  </FieldSet>
                </FieldGroup>
              </CardContent>
              <CardFooter>
                <span className="text-muted-foreground">
                  {t("Tier priority {priority} · {strategy}", {
                    priority: tier.priority,
                    strategy: selectionStrategyLabel(
                      tier.selection_strategy,
                    ),
                  })}
                </span>
              </CardFooter>
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
