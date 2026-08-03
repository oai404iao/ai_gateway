import { useState } from "react";
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@/app/i18n-provider";
import {
  RoutingTargetFields,
  type RoutingTargetChannel,
  type RoutingTargetGroup,
} from "@/components/shared/routing-target-fields";

const GROUPS: RoutingTargetGroup[] = [
  {
    id: "chat-later",
    name: "chat-later",
    api_format: "open_ai_chat_completions",
    enabled: true,
    priority: 5,
  },
  {
    id: "images-disabled",
    name: "images-disabled",
    api_format: "open_ai_images",
    enabled: false,
    priority: 1,
  },
  {
    id: "responses",
    name: "responses",
    api_format: "open_ai_responses",
    enabled: true,
    priority: 1,
  },
  {
    id: "chat-first",
    name: "chat-first",
    api_format: "open_ai_chat_completions",
    enabled: true,
    priority: 1,
  },
];

const CHANNELS: RoutingTargetChannel[] = [
  {
    id: "channel-enabled",
    channel_group_id: "chat-first",
    channel_group_name: "chat-first",
    channel_group_enabled: true,
    name: "channel-enabled",
    api_format: "open_ai_chat_completions",
    enabled: true,
    auto_disabled: false,
  },
  {
    id: "channel-disabled",
    channel_group_id: "hidden-disabled-group",
    channel_group_name: "hidden-disabled-group",
    channel_group_enabled: false,
    name: "channel-disabled",
    api_format: "open_ai_responses",
    enabled: true,
    auto_disabled: false,
  },
];

function Harness() {
  const [groupIds, setGroupIds] = useState<string[]>([]);
  const [channelIds, setChannelIds] = useState<string[]>([]);
  return (
    <RoutingTargetFields
      groups={GROUPS}
      channels={CHANNELS}
      selectedGroupIds={groupIds}
      selectedChannelIds={channelIds}
      onChange={(nextGroupIds, nextChannelIds) => {
        setGroupIds(nextGroupIds);
        setChannelIds(nextChannelIds);
      }}
    />
  );
}

describe("RoutingTargetFields", () => {
  it("groups and sorts targets, hides disabled targets, and keeps channels advanced", async () => {
    const user = userEvent.setup();
    render(
      <I18nProvider>
        <Harness />
      </I18nProvider>,
    );

    const groupCheckboxes = screen
      .getAllByRole("checkbox")
      .filter((checkbox) => checkbox.getAttribute("aria-label")?.includes("("));
    expect(groupCheckboxes.map((checkbox) => checkbox.getAttribute("aria-label"))).toEqual([
      "chat-first (Chat Completions)",
      "chat-later (Chat Completions)",
      "responses (Responses)",
    ]);
    expect(
      screen.queryByRole("checkbox", { name: "images-disabled (Images)" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", {
        name: "channel-enabled (chat-first)",
      }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByText("chat-first"));
    expect(screen.getByText("1 groups selected")).toBeInTheDocument();
    await user.click(screen.getByText("chat-first"));

    await user.click(
      screen.getByRole("button", { name: "Show individual channels (1)" }),
    );
    expect(
      screen.getByRole("checkbox", {
        name: "channel-enabled (chat-first)",
      }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", {
        name: "channel-disabled (hidden-disabled-group)",
      }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "Show disabled targets (2)" }));
    expect(
      screen.getByRole("checkbox", { name: "images-disabled (Images)" }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.getByRole("checkbox", {
        name: "channel-disabled (hidden-disabled-group)",
      }),
    ).toHaveAttribute("aria-disabled", "true");
  });
});
