import { useId, useMemo } from "react";
import { Plus, Trash2 } from "lucide-react";
import type {
  ChannelGroupView,
  ChannelView,
  ModelRuleInput,
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
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldContent,
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

type RoutingTier = ModelRuleInput["routing_tiers"][number];
type GroupTarget = RoutingTier["channel_groups"][number];
type ChannelWeight = GroupTarget["channels"][number];

interface ModelRuleTierEditorProps {
  value: RoutingTier[];
  groups: ChannelGroupView[];
  channels: ChannelView[];
  onChange: (value: RoutingTier[]) => void;
  errorFor: (path: Array<string | number>) => string | undefined;
  className?: string;
}

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

function channelWeight(
  channels: ChannelWeight[],
  channelId: string,
): ChannelWeight | undefined {
  return channels.find((channel) => channel.channel_id === channelId);
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
  const selectedGroupIds = useMemo(
    () =>
      new Set(
        value.flatMap((tier) =>
          tier.channel_groups.map((target) => target.channel_group_id),
        ),
      ),
    [value],
  );
  const availableGroups = groups
    .filter((group) => !selectedGroupIds.has(group.id))
    .sort((left, right) => left.name.localeCompare(right.name));

  const patchTier = (tierIndex: number, partial: Partial<RoutingTier>) => {
    onChange(updateAt(value, tierIndex, { ...value[tierIndex], ...partial }));
  };

  const patchTarget = (
    tierIndex: number,
    targetIndex: number,
    partial: Partial<GroupTarget>,
  ) => {
    const tier = value[tierIndex];
    patchTier(tierIndex, {
      channel_groups: updateAt(tier.channel_groups, targetIndex, {
        ...tier.channel_groups[targetIndex],
        ...partial,
      }),
    });
  };

  const setChannelSelected = (
    tierIndex: number,
    targetIndex: number,
    channelId: string,
    checked: boolean,
  ) => {
    const target = value[tierIndex].channel_groups[targetIndex];
    const nextChannels = checked
      ? channelWeight(target.channels, channelId)
        ? target.channels
        : [...target.channels, { channel_id: channelId, weight: 100 }]
      : target.channels.filter((channel) => channel.channel_id !== channelId);
    patchTarget(tierIndex, targetIndex, { channels: nextChannels });
  };

  const setChannelWeight = (
    tierIndex: number,
    targetIndex: number,
    channelId: string,
    weight: number,
  ) => {
    const target = value[tierIndex].channel_groups[targetIndex];
    patchTarget(tierIndex, targetIndex, {
      channels: target.channels.map((channel) =>
        channel.channel_id === channelId ? { ...channel, weight } : channel,
      ),
    });
  };

  const switchSelection = (
    tierIndex: number,
    targetIndex: number,
    selection: GroupTarget["channel_selection"],
  ) => {
    const target = value[tierIndex].channel_groups[targetIndex];
    if (selection === "all") {
      patchTarget(tierIndex, targetIndex, {
        channel_selection: "all",
        default_weight: 100,
        channels: target.channels.filter((channel) => channel.weight !== 100),
      });
      return;
    }

    const groupChannels = channels.filter(
      (channel) => channel.channel_group_id === target.channel_group_id,
    );
    const knownChannelIds = new Set(groupChannels.map((channel) => channel.id));
    const explicitChannels = [
      ...groupChannels.map((channel) => ({
        channel_id: channel.id,
        weight:
          channelWeight(target.channels, channel.id)?.weight ??
          target.default_weight ??
          100,
      })),
      ...target.channels.filter(
        (channel) => !knownChannelIds.has(channel.channel_id),
      ),
    ];
    patchTarget(tierIndex, targetIndex, {
      channel_selection: "selected",
      default_weight: null,
      channels: explicitChannels,
    });
  };

  return (
    <FieldSet
      className={cn(className)}
      data-invalid={Boolean(errorFor(["routing_tiers"])) || undefined}
    >
      <FieldLegend>{t("Rule routing tiers")}</FieldLegend>
      <FieldDescription>
        {t(
          "Lower priority tiers are tried first. Strategy and weights belong to this model rule, not to global channels or groups.",
        )}
      </FieldDescription>

      <FieldGroup>
        {value.map((tier, tierIndex) => {
          const tierPath = ["routing_tiers", tierIndex];
          return (
            <Card key={tierIndex}>
              <CardHeader>
                <CardTitle>
                  {t("Tier {number}", { number: tierIndex + 1 })}
                </CardTitle>
                <CardDescription>
                  {t("{count} channel groups", {
                    count: tier.channel_groups.length,
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
                          "Weights are compared only among eligible channels in this tier.",
                        )}
                      </FieldDescription>
                    </Field>
                  </FieldGroup>

                  <Field
                    data-invalid={
                      Boolean(errorFor([...tierPath, "channel_groups"])) ||
                      undefined
                    }
                  >
                    <FieldLabel>
                      {t("Add a format-compatible channel group")}
                    </FieldLabel>
                    <Select
                      value={ADD_GROUP_VALUE}
                      disabled={availableGroups.length === 0}
                      onValueChange={(groupId) => {
                        if (groupId === ADD_GROUP_VALUE) return;
                        patchTier(tierIndex, {
                          channel_groups: [
                            ...tier.channel_groups,
                            {
                              channel_group_id: groupId,
                              channel_selection: "all",
                              default_weight: 100,
                              channels: [],
                            },
                          ],
                        });
                      }}
                    >
                      <SelectTrigger
                        aria-label={t(
                          "Add channel group to tier {number}",
                          { number: tierIndex + 1 },
                        )}
                        aria-invalid={Boolean(
                          errorFor([...tierPath, "channel_groups"]),
                        )}
                      >
                        <SelectValue
                          placeholder={
                            availableGroups.length > 0
                              ? t("Choose a channel group")
                              : t("All compatible groups are already used")
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
                    {errorFor([...tierPath, "channel_groups"]) ? (
                      <FieldError>
                        {errorFor([...tierPath, "channel_groups"])}
                      </FieldError>
                    ) : null}
                  </Field>

                  {tier.channel_groups.map((target, targetIndex) => {
                    const targetPath = [
                      ...tierPath,
                      "channel_groups",
                      targetIndex,
                    ];
                    const group = groupById.get(target.channel_group_id);
                    const knownChannels = channels.filter(
                      (channel) =>
                        channel.channel_group_id === target.channel_group_id,
                    );
                    const missingChannels = target.channels
                      .filter(
                        (channel) => !channelById.has(channel.channel_id),
                      )
                      .map((channel) => ({
                        id: channel.channel_id,
                        name: channel.channel_id,
                      }));
                    const targetChannels = [
                      ...knownChannels.map((channel) => ({
                        id: channel.id,
                        name: channel.name,
                      })),
                      ...missingChannels,
                    ];
                    return (
                      <Card key={target.channel_group_id} size="sm">
                        <CardHeader>
                          <CardTitle>
                            {group?.name ?? target.channel_group_id}
                          </CardTitle>
                          <CardDescription>
                            {target.channel_selection === "all"
                              ? t(
                                  "All channels use the default weight unless overridden.",
                                )
                              : t(
                                  "Only explicitly selected channels are eligible.",
                                )}
                          </CardDescription>
                          <CardAction>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Remove channel group {name}", {
                                name:
                                  group?.name ?? target.channel_group_id,
                              })}
                              onClick={() =>
                                patchTier(tierIndex, {
                                  channel_groups: removeAt(
                                    tier.channel_groups,
                                    targetIndex,
                                  ),
                                })
                              }
                            >
                              <Trash2 data-icon="inline-start" />
                            </Button>
                          </CardAction>
                        </CardHeader>
                        <CardContent>
                          <FieldGroup>
                            {errorFor([
                              ...targetPath,
                              "channel_group_id",
                            ]) ? (
                              <FieldError>
                                {errorFor([
                                  ...targetPath,
                                  "channel_group_id",
                                ])}
                              </FieldError>
                            ) : null}
                            <Field
                              data-invalid={
                                Boolean(
                                  errorFor([
                                    ...targetPath,
                                    "channel_selection",
                                  ]),
                                ) || undefined
                              }
                            >
                              <FieldLabel>{t("Channel selection")}</FieldLabel>
                              <Select
                                value={target.channel_selection}
                                onValueChange={(selection) =>
                                  switchSelection(
                                    tierIndex,
                                    targetIndex,
                                    selection as GroupTarget["channel_selection"],
                                  )
                                }
                              >
                                <SelectTrigger
                                  aria-label={t(
                                    "Channel selection for {name}",
                                    {
                                      name:
                                        group?.name ??
                                        target.channel_group_id,
                                    },
                                  )}
                                  aria-invalid={Boolean(
                                    errorFor([
                                      ...targetPath,
                                      "channel_selection",
                                    ]),
                                  )}
                                >
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  <SelectGroup>
                                    <SelectItem value="all">
                                      {t("All channels")}
                                    </SelectItem>
                                    <SelectItem value="selected">
                                      {t("Selected channels")}
                                    </SelectItem>
                                  </SelectGroup>
                                </SelectContent>
                              </Select>
                            </Field>

                            {target.channel_selection === "all" ? (
                              <Field
                                data-invalid={
                                  Boolean(
                                    errorFor([
                                      ...targetPath,
                                      "default_weight",
                                    ]),
                                  ) || undefined
                                }
                              >
                                <FieldLabel
                                  htmlFor={`${idPrefix}-tier-${tierIndex}-target-${targetIndex}-default-weight`}
                                >
                                  {t("Default weight")}
                                </FieldLabel>
                                <Input
                                  id={`${idPrefix}-tier-${tierIndex}-target-${targetIndex}-default-weight`}
                                  type="number"
                                  min={1}
                                  max={2_147_483_647}
                                  step={1}
                                  value={target.default_weight ?? ""}
                                  aria-invalid={Boolean(
                                    errorFor([
                                      ...targetPath,
                                      "default_weight",
                                    ]),
                                  )}
                                  onChange={(event) =>
                                    patchTarget(tierIndex, targetIndex, {
                                      default_weight: Number(
                                        event.target.value,
                                      ),
                                    })
                                  }
                                />
                                <FieldDescription>
                                  {t(
                                    "Enable a channel below only to override this weight.",
                                  )}
                                </FieldDescription>
                                {errorFor([
                                  ...targetPath,
                                  "default_weight",
                                ]) ? (
                                  <FieldError>
                                    {errorFor([
                                      ...targetPath,
                                      "default_weight",
                                    ])}
                                  </FieldError>
                                ) : null}
                              </Field>
                            ) : null}

                            <FieldSet
                              data-invalid={
                                Boolean(
                                  errorFor([...targetPath, "channels"]),
                                ) || undefined
                              }
                            >
                              <FieldLegend variant="label">
                                {target.channel_selection === "all"
                                  ? t("Per-channel overrides")
                                  : t("Selected channels and weights")}
                              </FieldLegend>
                              <FieldDescription>
                                {target.channel_selection === "all"
                                  ? t(
                                      "Unchecked channels inherit the default weight.",
                                    )
                                  : t(
                                      "Select at least one channel and assign a positive weight.",
                                    )}
                              </FieldDescription>
                              {targetChannels.length > 0 ? (
                                <FieldGroup data-slot="checkbox-group">
                                  {targetChannels.map((channel) => {
                                    const selected = channelWeight(
                                      target.channels,
                                      channel.id,
                                    );
                                    const channelIndex =
                                      target.channels.findIndex(
                                        (item) =>
                                          item.channel_id === channel.id,
                                      );
                                    const channelPath = [
                                      ...targetPath,
                                      "channels",
                                      channelIndex,
                                    ];
                                    const weightError = selected
                                      ? errorFor([...channelPath, "weight"])
                                      : undefined;
                                    const inputId = `${idPrefix}-tier-${tierIndex}-target-${targetIndex}-channel-${channel.id}`;
                                    return (
                                      <Field
                                        key={channel.id}
                                        orientation="horizontal"
                                        data-invalid={
                                          Boolean(weightError) || undefined
                                        }
                                      >
                                        <Checkbox
                                          id={inputId}
                                          checked={Boolean(selected)}
                                          aria-invalid={Boolean(weightError)}
                                          onCheckedChange={(checked) =>
                                            setChannelSelected(
                                              tierIndex,
                                              targetIndex,
                                              channel.id,
                                              Boolean(checked),
                                            )
                                          }
                                        />
                                        <FieldContent>
                                          <FieldLabel
                                            htmlFor={inputId}
                                            className="font-normal"
                                          >
                                            {channel.name}
                                          </FieldLabel>
                                        </FieldContent>
                                        {selected ? (
                                          <Input
                                            className="max-w-28"
                                            type="number"
                                            min={1}
                                            max={2_147_483_647}
                                            step={1}
                                            value={selected.weight}
                                            aria-label={t(
                                              "Weight for channel {name}",
                                              { name: channel.name },
                                            )}
                                            aria-invalid={Boolean(weightError)}
                                            onChange={(event) =>
                                              setChannelWeight(
                                                tierIndex,
                                                targetIndex,
                                                channel.id,
                                                Number(event.target.value),
                                              )
                                            }
                                          />
                                        ) : null}
                                        {weightError ? (
                                          <FieldError>{weightError}</FieldError>
                                        ) : null}
                                      </Field>
                                    );
                                  })}
                                </FieldGroup>
                              ) : (
                                <FieldDescription>
                                  {t("This group has no channels.")}
                                </FieldDescription>
                              )}
                              {errorFor([...targetPath, "channels"]) ? (
                                <FieldError>
                                  {errorFor([...targetPath, "channels"])}
                                </FieldError>
                              ) : null}
                            </FieldSet>
                          </FieldGroup>
                        </CardContent>
                        <CardFooter className="justify-between gap-2">
                          <Badge variant="outline">
                            {target.channel_selection === "all"
                              ? t("All channels")
                              : t("{count} selected", {
                                  count: target.channels.length,
                                })}
                          </Badge>
                          <span className="text-muted-foreground">
                            {t("{count} channel overrides", {
                              count:
                                target.channel_selection === "all"
                                  ? target.channels.length
                                  : 0,
                            })}
                          </span>
                        </CardFooter>
                      </Card>
                    );
                  })}
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
              channel_groups: [],
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
