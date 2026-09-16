import { useState } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ModelProtocolRuleInput } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { CHANNEL, CHANNEL_GROUP } from "@/test/fixtures";
import { seedAuthenticatedSession } from "@/test/msw";
import { ModelRuleTierEditor } from "./model-rule-tier-editor";

const channels = [
  { ...CHANNEL, name: "Primary", available_models: ["wire-a", "wire-b"] },
  {
    ...CHANNEL,
    id: "second",
    name: "Secondary",
    available_models: ["wire-a", "wire-c"],
  },
  { ...CHANNEL, id: "third", name: "Third", available_models: ["wire-d"] },
];
type Tier = ModelProtocolRuleInput["routing_tiers"][number];

function Editor({ initial }: { initial?: Tier[] }) {
  const [value, setValue] = useState<Tier[]>(
    initial ?? [
      {
        priority: 0,
        selection_strategy: "weighted_random",
        candidates: [
          { channel_id: CHANNEL.id, upstream_model: "wire-a", weight: 3 },
        ],
      },
    ],
  );
  return (
    <AppProviders>
      <ModelRuleTierEditor
        value={value}
        groups={[CHANNEL_GROUP]}
        channels={channels}
        onChange={setValue}
        errorFor={() => undefined}
      />
    </AppProviders>
  );
}

describe("ModelRuleTierEditor", () => {
  beforeEach(() => seedAuthenticatedSession());
  it("searches channels and models, retains compatible models, and clears incompatible ones", async () => {
    const user = userEvent.setup();
    render(<Editor />);
    const channel = screen.getByRole("combobox", {
      name: "Channel for tier 1 row 1",
    });
    const model = screen.getByRole("combobox", {
      name: "Upstream model for tier 1 row 1",
    });
    await user.clear(channel);
    await user.type(channel, "Sec");
    expect(
      screen.queryByRole("option", { name: /Primary/ }),
    ).not.toBeInTheDocument();
    await user.click(await screen.findByRole("option", { name: /Secondary/ }));
    expect(model).toHaveValue("wire-a");
    expect(
      screen.getByRole("spinbutton", { name: "Weight for tier 1 row 1" }),
    ).toHaveValue(3);

    await user.click(channel);
    await user.clear(channel);
    expect(channel).toHaveValue("");
    await user.type(channel, "Third");
    expect(channel).toHaveValue("Third");
    await user.click(await screen.findByRole("option", { name: /Third/ }));
    expect(model).toHaveValue("");
    await user.type(model, "wire-d");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(model).toHaveValue("wire-d");
    await user.click(screen.getByRole("button", { name: "Add record" }));
    expect(model).toHaveValue("wire-d");
  });

  it("clears a model on channel changes if the resulting pair already exists", async () => {
    const user = userEvent.setup();
    render(
      <Editor
        initial={[
          {
            priority: 0,
            selection_strategy: "weighted_random",
            candidates: [
              { channel_id: CHANNEL.id, upstream_model: "wire-a", weight: 3 },
              { channel_id: "second", upstream_model: "wire-a", weight: 2 },
            ],
          },
        ]}
      />,
    );
    await user.click(
      screen.getByRole("combobox", { name: "Channel for tier 1 row 1" }),
    );
    await user.click(await screen.findByRole("option", { name: /Secondary/ }));
    expect(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }),
    ).toHaveValue("");
    await user.click(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }),
    );
    expect(
      await screen.findByRole("option", { name: "wire-a" }),
    ).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("option", { name: "wire-c" }));
    expect(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }),
    ).toHaveValue("wire-a");
  });

  it("keeps focus while editing weights and preserves the remaining row after removal", async () => {
    const user = userEvent.setup();
    render(<Editor />);
    const weight = screen.getByRole("spinbutton", {
      name: "Weight for tier 1 row 1",
    });
    await user.clear(weight);
    await user.type(weight, "125");
    expect(weight).toHaveFocus();
    expect(weight).toHaveValue(125);
    await user.click(screen.getByRole("button", { name: "Add record" }));
    await user.click(
      screen.getByRole("combobox", { name: "Channel for tier 1 row 2" }),
    );
    await user.click(await screen.findByRole("option", { name: /Third/ }));
    await user.click(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }),
    );
    await user.click(await screen.findByRole("option", { name: "wire-d" }));
    await user.click(
      screen.getByRole("button", { name: "Remove tier 1 row 1" }),
    );
    expect(
      screen.getByRole("combobox", { name: "Channel for tier 1 row 1" }),
    ).toHaveValue("Third");
    expect(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }),
    ).toHaveValue("wire-d");
    expect(
      screen.getByRole("spinbutton", { name: "Weight for tier 1 row 1" }),
    ).toHaveValue(1);
  });

  it("allows the same channel/model pair in another tier and retains unavailable saved values", async () => {
    const user = userEvent.setup();
    render(
      <Editor
        initial={[
          {
            priority: 0,
            selection_strategy: "weighted_random",
            candidates: [
              {
                channel_id: CHANNEL.id,
                upstream_model: "retired-model",
                weight: 3,
              },
            ],
          },
          {
            priority: 1,
            selection_strategy: "weighted_random",
            candidates: [
              { channel_id: CHANNEL.id, upstream_model: "wire-a", weight: 1 },
            ],
          },
        ]}
      />,
    );
    expect(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }),
    ).toHaveValue("retired-model");
    await user.click(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 1" }),
    );
    expect(
      await screen.findByRole("option", { name: "wire-a" }),
    ).not.toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("option", { name: "wire-a" }));
    await user.click(screen.getByRole("button", { name: "Remove tier 1" }));
    expect(
      screen.getByRole("spinbutton", { name: "Weight for tier 1 row 1" }),
    ).toHaveValue(1);
  });
});
