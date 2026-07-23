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
        channel_group_ids: [CHANNEL_GROUP.id],
        channel_ids: [],
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

    expect(plan.drafts[0]?.channel_group_ids).toEqual([]);
    expect(plan.drafts[0]?.channel_ids).toEqual([CHANNEL.id]);
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
    expect(plan.drafts[0]?.channel_group_ids).toEqual([RESPONSES_GROUP.id]);
  });
});
