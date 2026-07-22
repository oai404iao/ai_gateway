import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL, CHANNEL_GROUP } from "@/test/fixtures";
import type { ChannelGroupView, ChannelView } from "@/api/types";

const RESPONSES_GROUP: ChannelGroupView = {
  ...CHANNEL_GROUP,
  id: "00000000-0000-0000-0000-000000000023",
  name: "responses-backup",
  api_format: "open_ai_responses",
  priority: 2,
  selection_strategy: "weighted_round_robin",
};

const RESPONSES_CHANNEL: ChannelView = {
  ...CHANNEL,
  id: "00000000-0000-0000-0000-000000000024",
  channel_group_id: RESPONSES_GROUP.id,
  api_format: RESPONSES_GROUP.api_format,
  name: "responses-upstream",
  base_url: "https://responses.example",
};

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("ChannelsPage", () => {
  it("groups channels under their channel groups", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([CHANNEL_GROUP, RESPONSES_GROUP]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([RESPONSES_CHANNEL, CHANNEL]),
      ),
    );
    renderAppAt("/admin/routing/channels");

    const chatGroup = await screen.findByRole("region", { name: CHANNEL_GROUP.name });
    const responsesGroup = screen.getByRole("region", { name: RESPONSES_GROUP.name });

    expect(within(chatGroup).getByText(CHANNEL.name)).toBeInTheDocument();
    expect(within(chatGroup).queryByText(RESPONSES_CHANNEL.name)).not.toBeInTheDocument();
    expect(within(responsesGroup).getByText(RESPONSES_CHANNEL.name)).toBeInTheDocument();
    expect(within(responsesGroup).queryByText(CHANNEL.name)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New group" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New channel" })).toBeInTheDocument();
  });
});
