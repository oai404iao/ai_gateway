import type {
  ApiFormat,
  ChannelGroupView,
  ChannelView,
  ControlPlaneModel,
  ModelRuleInput,
  ModelRuleView,
} from "@/api/types";
import { API_FORMATS } from "@/lib/permissions";

export type QuickAddFormatStatus =
  | "ready"
  | "configured"
  | "no_channels"
  | "model_disabled";

export interface QuickAddFormatPlan {
  apiFormat: ApiFormat;
  status: QuickAddFormatStatus;
  compatibleChannelCount: number;
  input?: ModelRuleInput;
}

export interface QuickAddModelPlan {
  model: ControlPlaneModel;
  formats: QuickAddFormatPlan[];
  drafts: ModelRuleInput[];
}

interface TargetPlan {
  channelGroups: ModelRuleInput["routing_tiers"][number]["channel_groups"];
  compatibleChannelCount: number;
}

function ruleKey(clientModel: string, apiFormat: ApiFormat): string {
  return `${apiFormat}\u0000${clientModel}`;
}

function compareGroups(left: ChannelGroupView, right: ChannelGroupView): number {
  return left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
}

function compareChannels(left: ChannelView, right: ChannelView): number {
  return left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
}

function buildTargets(
  model: ControlPlaneModel,
  apiFormat: ApiFormat,
  groups: readonly ChannelGroupView[],
  channels: readonly ChannelView[],
): TargetPlan {
  const eligibleGroups = groups
    .filter((group) => group.enabled && group.api_format === apiFormat)
    .sort(compareGroups);
  const groupsById = new Map(eligibleGroups.map((group) => [group.id, group]));
  const channelsByGroup = new Map<string, ChannelView[]>();

  for (const channel of channels) {
    if (
      !channel.enabled ||
      channel.api_format !== apiFormat ||
      !groupsById.has(channel.channel_group_id)
    ) {
      continue;
    }
    const current = channelsByGroup.get(channel.channel_group_id) ?? [];
    current.push(channel);
    channelsByGroup.set(channel.channel_group_id, current);
  }

  const channelGroups: TargetPlan["channelGroups"] = [];
  let compatibleChannelCount = 0;

  for (const group of eligibleGroups) {
    const enabledChannels = (channelsByGroup.get(group.id) ?? []).sort(compareChannels);
    const compatibleChannels = enabledChannels.filter((channel) =>
      channel.available_models.includes(model.source_model_id),
    );
    if (compatibleChannels.length === 0) continue;

    compatibleChannelCount += compatibleChannels.length;
    if (compatibleChannels.length === enabledChannels.length) {
      channelGroups.push({
        channel_group_id: group.id,
        channel_selection: "all",
        default_weight: 100,
        channels: [],
      });
    } else {
      channelGroups.push({
        channel_group_id: group.id,
        channel_selection: "selected",
        default_weight: null,
        channels: compatibleChannels.map((channel) => ({
          channel_id: channel.id,
          weight: 100,
        })),
      });
    }
  }

  return {
    channelGroups,
    compatibleChannelCount,
  };
}

export function buildQuickAddModelPlans(
  models: readonly ControlPlaneModel[],
  groups: readonly ChannelGroupView[],
  channels: readonly ChannelView[],
  rules: readonly ModelRuleView[],
): QuickAddModelPlan[] {
  const configuredRuleKeys = new Set(
    rules.map((rule) => ruleKey(rule.client_model, rule.api_format)),
  );

  return models.map((model) => {
    const formats = API_FORMATS.map((apiFormat): QuickAddFormatPlan => {
      const targets = buildTargets(model, apiFormat, groups, channels);
      if (targets.compatibleChannelCount === 0) {
        return {
          apiFormat,
          status: "no_channels",
          compatibleChannelCount: 0,
        };
      }
      if (!model.enabled) {
        return {
          apiFormat,
          status: "model_disabled",
          compatibleChannelCount: targets.compatibleChannelCount,
        };
      }
      if (configuredRuleKeys.has(ruleKey(model.source_model_id, apiFormat))) {
        return {
          apiFormat,
          status: "configured",
          compatibleChannelCount: targets.compatibleChannelCount,
        };
      }
      return {
        apiFormat,
        status: "ready",
        compatibleChannelCount: targets.compatibleChannelCount,
        input: {
          client_model: model.source_model_id,
          api_format: apiFormat,
          upstream_model_id: model.id,
          description: null,
          routing_tiers: [
            {
              priority: 0,
              selection_strategy: "weighted_random",
              channel_groups: targets.channelGroups,
            },
          ],
          enabled: true,
        },
      };
    });

    return {
      model,
      formats,
      drafts: formats.flatMap((format) => (format.input ? [format.input] : [])),
    };
  });
}
