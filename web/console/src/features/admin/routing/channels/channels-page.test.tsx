import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL, CHANNEL_GROUP } from "@/test/fixtures";
import type {
  ChannelBatchUpdateInput,
  ChannelGroupView,
  ChannelView,
} from "@/api/types";

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
    expect(screen.getAllByText("1.25")).toHaveLength(2);
  });

  it("submits an atomic batch update for the selected channels", async () => {
    seedAuthenticatedSession();
    let submitted: ChannelBatchUpdateInput | undefined;
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([CHANNEL_GROUP, RESPONSES_GROUP]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([CHANNEL, RESPONSES_CHANNEL]),
      ),
      http.post("/console/v1/routing/channels/batch", async ({ request }) => {
        submitted = (await request.json()) as ChannelBatchUpdateInput;
        return HttpResponse.json({
          updated_ids: [CHANNEL.id, RESPONSES_CHANNEL.id],
          correlation_id: "77777777-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/routing/channels");

    await user.click(
      await screen.findByRole("checkbox", { name: `Select ${CHANNEL.name}` }),
    );
    await user.click(
      screen.getByRole("checkbox", { name: `Select ${RESPONSES_CHANNEL.name}` }),
    );
    await user.click(screen.getByRole("button", { name: "Batch edit (2)" }));
    await user.type(screen.getByLabelText("Billing multiplier"), "1.75");
    await user.click(screen.getByRole("button", { name: "Update channels" }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.items).toEqual([
      { id: CHANNEL.id, updated_at: CHANNEL.updated_at },
      { id: RESPONSES_CHANNEL.id, updated_at: RESPONSES_CHANNEL.updated_at },
    ]);
    expect(submitted?.changes).toEqual({ billing_multiplier: "1.75" });
  });
});
