import { describe, expect, it } from "vitest";
import { CHANNEL, CHANNEL_GROUP, MODEL, MODEL_RULE } from "@/test/fixtures";
import type { ModelRuleView } from "@/api/types";
import {
  modelNeedsAttention,
  rulesForGroup,
  supplyEntries,
  supplyNeedsAttention,
  targetChannels,
} from "./configuration-graph";

describe("configuration relationships", () => {
  it("combines a Codex pool without combining its format switches", () => {
    const response = {
      ...CHANNEL_GROUP,
      id: "responses",
      api_format: "open_ai_responses" as const,
      connector_kind: "codex_oauth" as const,
      connector_pool_id: "pool",
    };
    const image = {
      ...response,
      id: "images",
      api_format: "open_ai_images" as const,
      enabled: false,
    };
    const [pool] = supplyEntries([image, response]);
    expect(pool.id).toBe(response.id);
    expect(pool.groups).toHaveLength(2);
    expect(pool.groups.find((group) => group.id === image.id)?.enabled).toBe(
      false,
    );
    expect(
      supplyNeedsAttention(pool, [
        { ...CHANNEL, channel_group_id: response.id },
      ]),
    ).toBe(true);
  });

  it("respects selected-channel targets, format boundaries and deduplication across tiers", () => {
    const tier = {
      ...MODEL_RULE.routing_tiers[0],
      channel_groups: [
        {
          channel_group_id: CHANNEL_GROUP.id,
          channel_selection: "selected" as const,
          default_weight: null,
          channels: [{ channel_id: CHANNEL.id, weight: 25 }],
        },
      ],
    };
    const rule: ModelRuleView = {
      ...MODEL_RULE,
      routing_tiers: [tier, { ...tier, priority: 10 }],
    };
    expect(
      targetChannels(rule, [
        CHANNEL,
        { ...CHANNEL, id: "not-selected" },
        { ...CHANNEL, api_format: "open_ai_responses" },
      ]),
    ).toEqual([CHANNEL]);
    expect(rulesForGroup([rule], CHANNEL_GROUP.id)).toEqual([rule]);
  });

  it("includes future channels in an all-channel target without claiming they are healthy", () => {
    const disabled = { ...CHANNEL, id: "disabled", enabled: false };
    expect(targetChannels(MODEL_RULE, [disabled])).toEqual([disabled]);
  });

  it("flags unused and disabled price records without treating price alone as publication", () => {
    expect(modelNeedsAttention(MODEL, [])).toBe(true);
    expect(modelNeedsAttention(MODEL, [MODEL_RULE])).toBe(false);
    expect(
      modelNeedsAttention({ ...MODEL, enabled: false }, [MODEL_RULE]),
    ).toBe(true);
  });
});
