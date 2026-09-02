import { describe, expect, it } from "vitest";
import { CHANNEL, CHANNEL_GROUP, MODEL_RULE } from "@/test/fixtures";
import { channelUpdateRoutingImpact } from "@/features/admin/routing/routing-validation";

describe("channelUpdateRoutingImpact", () => {
  it("warns when disabling the only active target makes a route temporarily unavailable", () => {
    expect(
      channelUpdateRoutingImpact(
        CHANNEL.id,
        { ...CHANNEL, enabled: false },
        [CHANNEL],
        [CHANNEL_GROUP],
        [
          {
            ...MODEL_RULE,
            routing_tiers: [
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
            ],
          },
        ],
      ),
    ).toEqual([
      expect.objectContaining({
        ruleId: MODEL_RULE.id,
        previousStatus: "ready",
        nextStatus: "temporarily_unavailable",
      }),
    ]);
  });

  it("does not warn when another active model-capable target remains", () => {
    const fallback = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000099",
      name: "upstream-b",
    };
    expect(
      channelUpdateRoutingImpact(
        CHANNEL.id,
        { ...CHANNEL, enabled: false },
        [CHANNEL, fallback],
        [CHANNEL_GROUP],
        [MODEL_RULE],
      ),
    ).toEqual([]);
  });

  it("warns when removing a model from an already disabled sole target disconnects the rule", () => {
    const disabled = { ...CHANNEL, enabled: false };
    expect(
      channelUpdateRoutingImpact(
        CHANNEL.id,
        { ...disabled, available_models: [] },
        [disabled],
        [CHANNEL_GROUP],
        [MODEL_RULE],
      ),
    ).toEqual([
      expect.objectContaining({
        previousStatus: "temporarily_unavailable",
        nextStatus: "disconnected",
      }),
    ]);
  });

  it("does not warn when an edit leaves an already unavailable route unchanged", () => {
    const disabled = { ...CHANNEL, enabled: false };
    expect(
      channelUpdateRoutingImpact(
        CHANNEL.id,
        disabled,
        [disabled],
        [CHANNEL_GROUP],
        [MODEL_RULE],
      ),
    ).toEqual([]);
  });

  it("ignores channels in the same group that are not selected by the tier", () => {
    const selected = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000099",
      name: "selected-upstream",
    };
    expect(
      channelUpdateRoutingImpact(
        CHANNEL.id,
        { ...CHANNEL, enabled: false },
        [CHANNEL, selected],
        [CHANNEL_GROUP],
        [
          {
            ...MODEL_RULE,
            routing_tiers: [
              {
                priority: 0,
                selection_strategy: "weighted_random",
                channel_groups: [
                  {
                    channel_group_id: CHANNEL_GROUP.id,
                    channel_selection: "selected",
                    default_weight: null,
                    channels: [{ channel_id: selected.id, weight: 100 }],
                  },
                ],
              },
            ],
          },
        ],
      ),
    ).toEqual([]);
  });
});
