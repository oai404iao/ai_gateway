import { describe, expect, it } from "vitest";
import { CHANNEL, CHANNEL_GROUP, MODEL_RULE } from "@/test/fixtures";
import { channelUpdateInvalidatesRouting } from "@/features/admin/routing/routing-validation";

describe("channelUpdateInvalidatesRouting", () => {
  it("blocks disabling the only direct target of an enabled model rule", () => {
    expect(
      channelUpdateInvalidatesRouting(
        CHANNEL.id,
        { ...CHANNEL, enabled: false },
        [CHANNEL],
        [CHANNEL_GROUP],
        [{ ...MODEL_RULE, channel_group_ids: [], channel_ids: [CHANNEL.id] }],
      ),
    ).toBe(true);
  });

  it("allows the update when another eligible target remains", () => {
    const fallback = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000099",
      name: "upstream-b",
    };
    expect(
      channelUpdateInvalidatesRouting(
        CHANNEL.id,
        { ...CHANNEL, enabled: false },
        [CHANNEL, fallback],
        [CHANNEL_GROUP],
        [{ ...MODEL_RULE, channel_group_ids: [CHANNEL_GROUP.id], channel_ids: [] }],
      ),
    ).toBe(false);
  });
});
