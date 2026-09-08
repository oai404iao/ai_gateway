import type {
  ChannelGroupView,
  ChannelView,
  ControlPlaneModel,
  ModelRuleView,
} from "@/api/types";
import { adminPath } from "./model-setup-navigation";

export type ConfigurationLens = "routes" | "supply" | "models";

export const LENS_PATHS: Record<ConfigurationLens, string> = {
  routes: "/admin/routing/model-rules",
  supply: "/admin/routing/channels",
  models: "/admin/models",
};

export function configurationPath(lens: ConfigurationLens, id?: string) {
  return adminPath(LENS_PATHS[lens], { selected: id });
}

export interface ConfigurationData {
  rules: ModelRuleView[];
  groups: ChannelGroupView[];
  channels: ChannelView[];
  models: ControlPlaneModel[];
}

export interface SupplyEntry {
  id: string;
  name: string;
  groups: ChannelGroupView[];
  managed: boolean;
}

/** A pool is one directory entry, but its format-specific groups stay separate. */
export function supplyEntries(groups: ChannelGroupView[]): SupplyEntry[] {
  const entries = new Map<string, SupplyEntry>();
  for (const group of groups) {
    const managed = group.connector_kind === "codex_oauth";
    const key = managed ? (group.connector_pool_id ?? group.id) : group.id;
    const entry = entries.get(key);
    if (entry) {
      entry.groups.push(group);
      if (group.api_format === "open_ai_responses") {
        entry.id = group.id;
        entry.name = group.name;
      }
    } else {
      entries.set(key, {
        id: group.id,
        name: group.name,
        groups: [group],
        managed,
      });
    }
  }
  return [...entries.values()].sort((a, b) => a.name.localeCompare(b.name));
}

export function rulesForGroup(rules: ModelRuleView[], groupId: string) {
  return rules.filter((rule) =>
    rule.routing_tiers.some((tier) =>
      tier.channel_groups.some((target) => target.channel_group_id === groupId),
    ),
  );
}

/** Target membership is not health or eligibility; the API owns routing status. */
export function targetChannels(rule: ModelRuleView, channels: ChannelView[]) {
  return channels.filter(
    (channel) =>
      channel.api_format === rule.api_format &&
      rule.routing_tiers.some((tier) =>
        tier.channel_groups.some(
          (target) =>
            target.channel_group_id === channel.channel_group_id &&
            (target.channel_selection === "all" ||
              target.channels.some((item) => item.channel_id === channel.id)),
        ),
      ),
  );
}

export function supplyNeedsAttention(
  entry: SupplyEntry,
  channels: ChannelView[],
) {
  return entry.groups.some(
    (group) =>
      !group.enabled ||
      !channels.some(
        (channel) =>
          channel.channel_group_id === group.id &&
          channel.enabled &&
          !channel.auto_disabled,
      ),
  );
}

export function modelNeedsAttention(
  model: ControlPlaneModel,
  rules: ModelRuleView[],
) {
  return (
    !model.enabled || !rules.some((rule) => rule.upstream_model_id === model.id)
  );
}
