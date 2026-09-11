import type {
  ChannelGroupView,
  ChannelView,
  ModelProtocolRuleView,
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
  protocolRuleId: string;
  clientModel: string;
  apiFormat: ModelProtocolRuleView["api_format"];
  previousStatus: ModelRuleRoutingStatus;
  nextStatus: ModelRuleRoutingStatus;
}

function routingStatus(
  rule: ModelRuleView,
  protocol: ModelProtocolRuleView,
  channels: readonly ChannelView[],
  groups: readonly ChannelGroupView[],
): ModelRuleRoutingStatus {
  if (!rule.model_enabled) return "model_disabled";
  if (!protocol.enabled) {
    return protocol.routing_tiers.length === 0 ? "draft" : "disabled";
  }
  if (protocol.routing_tiers.length === 0) return "draft";

  const groupsById = new Map(groups.map((group) => [group.id, group]));
  let modelCapableCount = 0;
  let activeCount = 0;
  for (const channel of channels) {
    const target = protocol.routing_tiers
      .flatMap((tier) => tier.channel_groups)
      .find(
        (entry) =>
          entry.channel_group_id === channel.channel_group_id &&
          (entry.channel_selection === "all" ||
            entry.channels.some(
              (selected) => selected.channel_id === channel.id,
            )),
      );
    if (channel.api_format !== protocol.api_format || !target) {
      continue;
    }
    const upstreamModel =
      target.channel_selection === "all"
        ? target.upstream_model
        : target.channels.find((entry) => entry.channel_id === channel.id)
            ?.upstream_model;
    if (!upstreamModel || !channel.available_models.includes(upstreamModel)) {
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
  if (activeCount > 0) return "ready";
  if (modelCapableCount > 0) return "temporarily_unavailable";
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
    case "draft":
    case "model_disabled":
    case "disabled":
      return -1;
  }
}

/**
 * Returns enabled protocol rules whose effective routing state would degrade
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

  return rules.flatMap((rule) =>
    rule.protocol_rules.flatMap((protocol) => {
      if (!protocol.enabled) return [];
      const previousStatus = routingStatus(rule, protocol, channels, groups);
      const nextStatus = routingStatus(
        rule,
        protocol,
        effectiveChannels,
        groups,
      );
      if (degradationRank(nextStatus) <= degradationRank(previousStatus)) {
        return [];
      }
      return [
        {
          protocolRuleId: protocol.id,
          clientModel: rule.client_model,
          apiFormat: protocol.api_format,
          previousStatus,
          nextStatus,
        },
      ];
    }),
  );
}
