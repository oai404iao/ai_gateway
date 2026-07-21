import type { ChannelGroupView, ChannelView, ModelRuleView } from "@/api/types";

interface ChannelDraft {
  channel_group_id: string;
  api_format: ChannelView["api_format"];
  enabled: boolean;
  available_models: string[];
}

function eligibleForRule(
  channel: ChannelView,
  rule: ModelRuleView,
  groups: ChannelGroupView[],
): boolean {
  const group = groups.find((item) => item.id === channel.channel_group_id);
  return (
    channel.enabled &&
    !channel.auto_disabled &&
    group?.enabled === true &&
    channel.api_format === rule.api_format &&
    group.api_format === rule.api_format &&
    channel.available_models.includes(rule.upstream_model)
  );
}

/**
 * Returns whether updating one channel would make a currently enabled model
 * rule invalid. The server remains authoritative; this only prevents the
 * common local edit that would otherwise end in an opaque-looking 422.
 */
export function channelUpdateInvalidatesRouting(
  channelId: string,
  draft: ChannelDraft,
  channels: ChannelView[],
  groups: ChannelGroupView[],
  rules: ModelRuleView[],
): boolean {
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
  const draftChannel = effectiveChannels.find((channel) => channel.id === channelId);
  if (!draftChannel) return false;

  return rules.some((rule) => {
    if (!rule.enabled) return false;
    if (
      rule.channel_ids.includes(channelId) &&
      !eligibleForRule(draftChannel, rule, groups)
    ) {
      return true;
    }
    if (
      rule.channel_ids.includes(channelId) &&
      rule.channel_group_ids.includes(draft.channel_group_id)
    ) {
      return true;
    }
    return !effectiveChannels.some(
      (channel) =>
        (rule.channel_ids.includes(channel.id) ||
          rule.channel_group_ids.includes(channel.channel_group_id)) &&
        eligibleForRule(channel, rule, groups),
    );
  });
}
