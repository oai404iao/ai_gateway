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
  ChannelGroupInput,
  ChannelGroupView,
  ChannelRecoverInput,
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

const DISABLED_CHANNEL: ChannelView = {
  ...RESPONSES_CHANNEL,
  id: "00000000-0000-0000-0000-000000000025",
  name: "disabled-upstream",
  enabled: false,
};

const AUTO_DISABLED_CHANNEL: ChannelView = {
  ...CHANNEL,
  id: "00000000-0000-0000-0000-000000000026",
  name: "auto-disabled-upstream",
  auto_disabled: true,
  auto_disabled_reason: "quota exceeded",
};

const CODEX_POOL_ID = "00000000-0000-0000-0000-000000000027";

const CODEX_RESPONSES_GROUP: ChannelGroupView = {
  ...CHANNEL_GROUP,
  id: CODEX_POOL_ID,
  name: "codex-subscriptions",
  api_format: "open_ai_responses",
  connector_kind: "codex_oauth",
  connector_pool_id: CODEX_POOL_ID,
};

const CODEX_IMAGES_GROUP: ChannelGroupView = {
  ...CODEX_RESPONSES_GROUP,
  id: "00000000-0000-0000-0000-000000000028",
  name: "codex-subscriptions Images",
  api_format: "open_ai_images",
  enabled: false,
};

const CODEX_RESPONSES_CHANNEL: ChannelView = {
  ...RESPONSES_CHANNEL,
  id: "00000000-0000-0000-0000-000000000029",
  channel_group_id: CODEX_RESPONSES_GROUP.id,
  connector_kind: "codex_oauth",
  provider_managed: true,
  name: "personal-plus",
};

const CODEX_IMAGES_CHANNEL: ChannelView = {
  ...CODEX_RESPONSES_CHANNEL,
  id: "00000000-0000-0000-0000-000000000030",
  channel_group_id: CODEX_IMAGES_GROUP.id,
  api_format: "open_ai_images",
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

  it("combines Codex Responses and Images groups from one credential pool", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([
          CHANNEL_GROUP,
          CODEX_RESPONSES_GROUP,
          CODEX_IMAGES_GROUP,
        ]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([
          CHANNEL,
          CODEX_RESPONSES_CHANNEL,
          CODEX_IMAGES_CHANNEL,
        ]),
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/routing/channels");

    const pool = await screen.findByRole("region", {
      name: CODEX_RESPONSES_GROUP.name,
    });
    expect(
      screen.getByRole("heading", { name: "Codex credential pools" }),
    ).toBeInTheDocument();
    expect(within(pool).getByText(CODEX_IMAGES_GROUP.name)).toBeInTheDocument();
    expect(within(pool).getByText("Responses")).toBeInTheDocument();
    expect(within(pool).getByText("Images")).toBeInTheDocument();
    expect(
      within(pool).getByRole("button", { name: "Manage shared credentials" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("region", { name: CODEX_IMAGES_GROUP.name }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Codex OAuth" }));
    expect(
      screen.queryByRole("region", { name: CHANNEL_GROUP.name }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: CODEX_RESPONSES_GROUP.name }),
    ).toBeInTheDocument();
  });

  it("filters a large group list and expands the matching channel", async () => {
    seedAuthenticatedSession();
    const largeGroups = Array.from({ length: 5 }, (_, index) => ({
      ...CHANNEL_GROUP,
      id: `00000000-0000-0000-0000-0000000001${index}`,
      name: index === 4 ? "target-group" : `bulk-group-${index}`,
      priority: index,
    }));
    const targetChannel: ChannelView = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000199",
      channel_group_id: largeGroups[4].id,
      name: "needle-upstream",
    };
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json(largeGroups),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([targetChannel]),
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/routing/channels");

    await user.type(
      await screen.findByRole("searchbox", {
        name: "Search groups or channels",
      }),
      "needle",
    );

    const targetGroup = await screen.findByRole("region", {
      name: "target-group",
    });
    expect(within(targetGroup).getByText("needle-upstream")).toBeVisible();
    expect(
      screen.queryByRole("region", { name: "bulk-group-0" }),
    ).not.toBeInTheDocument();
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

  it("quickly disables, enables, and recovers channels from the operations column", async () => {
    seedAuthenticatedSession();
    const batchInputs: ChannelBatchUpdateInput[] = [];
    let groupInput: ChannelGroupInput | undefined;
    let groupIfMatch: string | null = null;
    let recoverInput: ChannelRecoverInput | undefined;
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([CHANNEL_GROUP, RESPONSES_GROUP]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([CHANNEL, DISABLED_CHANNEL, AUTO_DISABLED_CHANNEL]),
      ),
      http.post("/console/v1/routing/channels/batch", async ({ request }) => {
        batchInputs.push((await request.json()) as ChannelBatchUpdateInput);
        return HttpResponse.json({
          updated_ids: [CHANNEL.id],
          correlation_id: "77777777-0000-0000-0000-000000000001",
        });
      }),
      http.put(
        "/console/v1/routing/channel-groups/:id",
        async ({ params, request }) => {
          expect(params.id).toBe(CHANNEL_GROUP.id);
          groupInput = (await request.json()) as ChannelGroupInput;
          groupIfMatch = request.headers.get("If-Match");
          return HttpResponse.json({
            id: CHANNEL_GROUP.id,
            correlation_id: "77777777-0000-0000-0000-000000000003",
          });
        },
      ),
      http.post(
        "/console/v1/routing/channels/:id/recover",
        async ({ params, request }) => {
          expect(params.id).toBe(AUTO_DISABLED_CHANNEL.id);
          recoverInput = (await request.json()) as ChannelRecoverInput;
          return HttpResponse.json({
            id: AUTO_DISABLED_CHANNEL.id,
            correlation_id: "77777777-0000-0000-0000-000000000002",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/routing/channels");

    const groupRegion = await screen.findByRole("region", {
      name: CHANNEL_GROUP.name,
    });
    await user.click(
      within(groupRegion).getByRole("button", { name: "Disable group" }),
    );
    const groupDialog = await screen.findByRole("alertdialog");
    await user.click(
      within(groupDialog).getByRole("button", { name: "Disable group" }),
    );
    await waitFor(() => expect(groupInput?.enabled).toBe(false));
    expect(groupInput).toEqual({
      name: CHANNEL_GROUP.name,
      api_format: CHANNEL_GROUP.api_format,
      connector_kind: CHANNEL_GROUP.connector_kind,
      priority: CHANNEL_GROUP.priority,
      selection_strategy: CHANNEL_GROUP.selection_strategy,
      enabled: false,
    });
    expect(groupIfMatch).toBe(`"${CHANNEL_GROUP.updated_at}"`);

    await user.click(
      await screen.findByRole("button", { name: `Disable ${CHANNEL.name}` }),
    );
    await user.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() => expect(batchInputs[0]?.changes).toEqual({ enabled: false }));

    await user.click(
      screen.getByRole("button", { name: `Enable ${DISABLED_CHANNEL.name}` }),
    );
    await waitFor(() => expect(batchInputs[1]?.changes).toEqual({ enabled: true }));

    await user.click(
      screen.getByRole("button", {
        name: `Recover ${AUTO_DISABLED_CHANNEL.name}`,
      }),
    );
    await waitFor(() => {
      expect(recoverInput).toEqual({
        updated_at: AUTO_DISABLED_CHANNEL.updated_at,
      });
    });
  });
});
