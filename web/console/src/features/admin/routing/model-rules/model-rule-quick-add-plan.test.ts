import { describe, expect, it } from "vitest";
import type { ChannelGroupView, ChannelView, ModelRuleView } from "@/api/types";
import { CHANNEL, CHANNEL_GROUP, MODEL, MODEL_RULE } from "@/test/fixtures";
import { buildQuickAddModelPlans } from "@/features/admin/routing/model-rules/model-rule-quick-add-plan";

const RESPONSES_GROUP: ChannelGroupView = {
  ...CHANNEL_GROUP,
  id: "00000000-0000-0000-0000-000000000023",
  name: "responses-primary",
  api_format: "open_ai_responses",
};

const RESPONSES_CHANNEL: ChannelView = {
  ...CHANNEL,
  id: "00000000-0000-0000-0000-000000000024",
  channel_group_id: RESPONSES_GROUP.id,
  api_format: RESPONSES_GROUP.api_format,
  name: "responses-upstream",
};

describe("buildQuickAddModelPlans", () => {
  it("uses a complete channel group when every enabled channel supports the model", () => {
    const [plan] = buildQuickAddModelPlans(
      [MODEL],
      [CHANNEL_GROUP],
      [CHANNEL],
      [],
    );

    expect(plan.drafts).toEqual([
      {
        client_model: MODEL.source_model_id,
        api_format: "open_ai_chat_completions",
        upstream_model_id: MODEL.id,
        description: null,
        routing_tiers: [
          {
            priority: 0,
            selection_strategy: "weighted_random",
            channel_groups: [
              {
                channel_group_id: CHANNEL_GROUP.id,
                channel_selection: "all",
                default_weight: 100,
                channels: [],
              },
            ],
          },
        ],
        enabled: true,
      },
    ]);
  });

  it("selects only compatible channels when a group contains mixed model support", () => {
    const incompatibleChannel: ChannelView = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000027",
      name: "other-model-only",
      available_models: ["another/model"],
    };

    const [plan] = buildQuickAddModelPlans(
      [MODEL],
      [CHANNEL_GROUP],
      [CHANNEL, incompatibleChannel],
      [],
    );

    expect(plan.drafts[0]?.routing_tiers).toEqual([
      {
        priority: 0,
        selection_strategy: "weighted_random",
        channel_groups: [
          {
            channel_group_id: CHANNEL_GROUP.id,
            channel_selection: "selected",
            default_weight: null,
            channels: [{ channel_id: CHANNEL.id, weight: 100 }],
          },
        ],
      },
    ]);
  });

  it("creates only missing API-format rules", () => {
    const existingRule: ModelRuleView = {
      ...MODEL_RULE,
      client_model: MODEL.source_model_id,
    };

    const [plan] = buildQuickAddModelPlans(
      [MODEL],
      [CHANNEL_GROUP, RESPONSES_GROUP],
      [CHANNEL, RESPONSES_CHANNEL],
      [existingRule],
    );

    expect(plan.drafts).toHaveLength(1);
    expect(plan.drafts[0]?.api_format).toBe("open_ai_responses");
    expect(plan.drafts[0]?.routing_tiers[0]?.channel_groups).toEqual([
      {
        channel_group_id: RESPONSES_GROUP.id,
        channel_selection: "all",
        default_weight: 100,
        channels: [],
      },
    ]);
  });
});
