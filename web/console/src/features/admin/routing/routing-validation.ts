import type {
  ChannelGroupView,
  ChannelView,
  ModelRuleRoutingStatus,
  ModelRuleView,
} from "@/api/types";

interface ChannelDraft {
  channel_group_id: string;
  api_format: ChannelView["api_format"];
  enabled: boolean;
  available_models: string[];
}

export interface ChannelRoutingImpact {
  ruleId: string;
  clientModel: string;
  apiFormat: ModelRuleView["api_format"];
  previousStatus: ModelRuleRoutingStatus;
  nextStatus: ModelRuleRoutingStatus;
}

function routingStatus(
  rule: ModelRuleView,
  channels: readonly ChannelView[],
  groups: readonly ChannelGroupView[],
): ModelRuleRoutingStatus {
  if (!rule.enabled) return "disabled";

  const groupsById = new Map(groups.map((group) => [group.id, group]));
  let modelCapableCount = 0;
  let activeCount = 0;
  for (const channel of channels) {
    if (
      channel.api_format !== rule.api_format ||
      (!rule.channel_ids.includes(channel.id) &&
        !rule.channel_group_ids.includes(channel.channel_group_id)) ||
      !channel.available_models.includes(rule.upstream_model)
    ) {
      continue;
    }
    modelCapableCount += 1;
    if (
      channel.enabled &&
      !channel.auto_disabled &&
      groupsById.get(channel.channel_group_id)?.enabled === true
    ) {
      activeCount += 1;
    }
  }
  if (rule.upstream_model_enabled && activeCount > 0) return "ready";
  if (rule.upstream_model_enabled && modelCapableCount > 0) {
    return "temporarily_unavailable";
  }
  return "disconnected";
}

function degradationRank(status: ModelRuleRoutingStatus): number {
  switch (status) {
    case "ready":
      return 0;
    case "temporarily_unavailable":
      return 1;
    case "disconnected":
      return 2;
    case "disabled":
      return -1;
  }
}

/**
 * Returns enabled model rules whose effective routing state would degrade
 * after updating one channel. This is a best-effort impact preview only:
 * degradation remains a valid administrator action and the server remains
 * authoritative for structural validation.
 */
export function channelUpdateRoutingImpact(
  channelId: string,
  draft: ChannelDraft,
  channels: readonly ChannelView[],
  groups: readonly ChannelGroupView[],
  rules: readonly ModelRuleView[],
): ChannelRoutingImpact[] {
  const effectiveChannels = channels.map((channel) =>
    channel.id === channelId
      ? {
          ...channel,
          channel_group_id: draft.channel_group_id,
          api_format: draft.api_format,
          enabled: draft.enabled,
          available_models: draft.available_models,
        }
      : channel,
  );
  if (!effectiveChannels.some((channel) => channel.id === channelId)) return [];

  return rules.flatMap((rule) => {
    if (!rule.enabled) return [];
    const previousStatus = routingStatus(rule, channels, groups);
    const nextStatus = routingStatus(rule, effectiveChannels, groups);
    if (degradationRank(nextStatus) <= degradationRank(previousStatus)) return [];
    return [
      {
        ruleId: rule.id,
        clientModel: rule.client_model,
        apiFormat: rule.api_format,
        previousStatus,
        nextStatus,
      },
    ];
  });
}
