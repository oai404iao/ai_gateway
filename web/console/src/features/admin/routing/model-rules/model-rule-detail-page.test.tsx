import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import {
  CHANNEL,
  CHANNEL_GROUP,
  MODEL,
  MODEL_RULE,
} from "@/test/fixtures";
import type {
  ChannelView,
  ModelRuleInput,
  ModelRuleView,
} from "@/api/types";

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

describe("ModelRuleDetailPage", () => {
  it("uses the shared Select UI for upstream and custom client models", async () => {
    seedAuthenticatedSession();
    let submitted: ModelRuleInput | undefined;
    server.use(
      http.put("/console/v1/routing/model-rules/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleInput;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    const clientModelSelect = await screen.findByRole("combobox", { name: "Client model" });
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.getByText("Active 1 · Capable 1 · Targets 1")).toBeInTheDocument();
    await user.click(clientModelSelect);
    const listbox = await screen.findByRole("listbox");
    expect(within(listbox).getByText(MODEL.provider_name ?? "")).toBeInTheDocument();
    await user.click(
      within(listbox).getByRole("option", {
        name: `${MODEL.display_name} (${MODEL.source_model_id})`,
      }),
    );

    await user.click(clientModelSelect);
    await user.click(await screen.findByRole("option", { name: "Custom client model" }));
    await user.type(
      await screen.findByRole("textbox", { name: "Custom client model" }),
      "my-custom-client-model",
    );
    await user.click(screen.getByRole("button", { name: /save rule/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.client_model).toBe("my-custom-client-model");
  });

  it("hydrates and serializes selected channels with rule-owned tier weights", async () => {
    seedAuthenticatedSession();
    const hydratedRule: ModelRuleView = {
      ...MODEL_RULE,
      routing_tiers: [
        {
          priority: 3,
          selection_strategy: "weighted_round_robin",
          channel_groups: [
            {
              channel_group_id: CHANNEL_GROUP.id,
              channel_selection: "selected",
              default_weight: null,
              channels: [{ channel_id: CHANNEL.id, weight: 47 }],
            },
          ],
        },
      ],
    };
    let submitted: ModelRuleInput | undefined;
    server.use(
      http.get("/console/v1/routing/model-rules/:id", () =>
        HttpResponse.json(hydratedRule, {
          headers: { ETag: `"${hydratedRule.updated_at}"` },
        }),
      ),
      http.put("/console/v1/routing/model-rules/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleInput;
        return HttpResponse.json({
          id: hydratedRule.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    expect(await screen.findByLabelText("Priority")).toHaveValue(3);
    expect(
      screen.getByRole("combobox", {
        name: "Selection strategy for tier 1",
      }),
    ).toHaveTextContent("Weighted round-robin");
    expect(
      screen.getByRole("combobox", {
        name: `Channel selection for ${CHANNEL_GROUP.name}`,
      }),
    ).toHaveTextContent("Selected channels");
    expect(screen.getByRole("checkbox", { name: CHANNEL.name })).toBeChecked();
    expect(
      screen.getByRole("spinbutton", {
        name: `Weight for channel ${CHANNEL.name}`,
      }),
    ).toHaveValue(47);

    await user.click(screen.getByRole("button", { name: /save rule/i }));
    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.routing_tiers).toEqual(hydratedRule.routing_tiers);
  });

  it("serializes all-channel defaults and optional channel overrides", async () => {
    seedAuthenticatedSession();
    let submitted: ModelRuleInput | undefined;
    server.use(
      http.put("/console/v1/routing/model-rules/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleInput;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    const defaultWeight = await screen.findByLabelText("Default weight");
    await user.clear(defaultWeight);
    await user.type(defaultWeight, "125");
    await user.click(screen.getByRole("checkbox", { name: CHANNEL.name }));
    const overrideWeight = screen.getByRole("spinbutton", {
      name: `Weight for channel ${CHANNEL.name}`,
    });
    await user.clear(overrideWeight);
    await user.type(overrideWeight, "75");
    await user.click(screen.getByRole("button", { name: /save rule/i }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.routing_tiers).toEqual([
      {
        priority: 0,
        selection_strategy: "weighted_random",
        channel_groups: [
          {
            channel_group_id: CHANNEL_GROUP.id,
            channel_selection: "all",
            default_weight: 125,
            channels: [{ channel_id: CHANNEL.id, weight: 75 }],
          },
        ],
      },
    ]);
  });

  it("keeps inherited weights when switching an all target to selected", async () => {
    seedAuthenticatedSession();
    const secondChannel: ChannelView = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000098",
      name: "upstream-b",
    };
    const allWithOverride: ModelRuleView = {
      ...MODEL_RULE,
      routing_tiers: [
        {
          priority: 0,
          selection_strategy: "weighted_random",
          channel_groups: [
            {
              channel_group_id: CHANNEL_GROUP.id,
              channel_selection: "all",
              default_weight: 125,
              channels: [{ channel_id: CHANNEL.id, weight: 75 }],
            },
          ],
        },
      ],
    };
    let submitted: ModelRuleInput | undefined;
    server.use(
      http.get("/console/v1/routing/model-rules/:id", () =>
        HttpResponse.json(allWithOverride, {
          headers: { ETag: `"${allWithOverride.updated_at}"` },
        }),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([CHANNEL, secondChannel]),
      ),
      http.put("/console/v1/routing/model-rules/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleInput;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    await user.click(
      await screen.findByRole("combobox", {
        name: `Channel selection for ${CHANNEL_GROUP.name}`,
      }),
    );
    await user.click(
      await screen.findByRole("option", { name: "Selected channels" }),
    );
    await user.click(screen.getByRole("button", { name: /save rule/i }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.routing_tiers[0]?.channel_groups[0]).toEqual({
      channel_group_id: CHANNEL_GROUP.id,
      channel_selection: "selected",
      default_weight: null,
      channels: [
        { channel_id: CHANNEL.id, weight: 75 },
        { channel_id: secondChannel.id, weight: 125 },
      ],
    });
  });

  it("renders tier and target validation errors without submitting", async () => {
    seedAuthenticatedSession();
    let putCount = 0;
    server.use(
      http.put("/console/v1/routing/model-rules/:id", () => {
        putCount += 1;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    await screen.findByLabelText("Priority");
    await user.click(screen.getByRole("button", { name: "Add routing tier" }));
    const priorities = screen.getAllByLabelText("Priority");
    await user.clear(priorities[1]);
    await user.type(priorities[1], "0");
    await user.click(screen.getByRole("button", { name: /save rule/i }));

    expect(await screen.findByText("Tier priorities must be unique.")).toBeVisible();
    expect(
      screen.getByText("Add at least one channel group to this tier."),
    ).toBeVisible();
    expect(putCount).toBe(0);
  });

  it("rejects routing integers larger than the backend representation", async () => {
    seedAuthenticatedSession();
    let putCount = 0;
    server.use(
      http.put("/console/v1/routing/model-rules/:id", () => {
        putCount += 1;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    const priority = await screen.findByLabelText("Priority");
    await user.clear(priority);
    await user.type(priority, "2147483648");
    const defaultWeight = screen.getByLabelText("Default weight");
    await user.clear(defaultWeight);
    await user.type(defaultWeight, "2147483648");
    await user.click(screen.getByRole("checkbox", { name: CHANNEL.name }));
    const channelWeight = screen.getByLabelText(
      `Weight for channel ${CHANNEL.name}`,
    );
    await user.clear(channelWeight);
    await user.type(channelWeight, "2147483648");
    await user.click(screen.getByRole("button", { name: /save rule/i }));

    expect(await screen.findByText("Priority is too large.")).toBeVisible();
    expect(screen.getByText("Default weight is too large.")).toBeVisible();
    expect(screen.getByText("Channel weights are too large.")).toBeVisible();
    expect(putCount).toBe(0);
  });

  it("prefills a partial group as selected compatible channels at weight 100", async () => {
    seedAuthenticatedSession();
    const incompatibleChannel: ChannelView = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000099",
      name: "upstream-incompatible",
      available_models: ["another-model"],
    };
    let submitted: ModelRuleInput | undefined;
    server.use(
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([CHANNEL, incompatibleChannel]),
      ),
      http.post("/console/v1/routing/model-rules", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleInput;
        return HttpResponse.json({
          id: MODEL_RULE.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(
      `/admin/routing/model-rules/new?upstreamModelId=${MODEL.id}&channelGroupId=${CHANNEL_GROUP.id}`,
    );

    expect(
      await screen.findByRole("combobox", {
        name: `Channel selection for ${CHANNEL_GROUP.name}`,
      }),
    ).toHaveTextContent("Selected channels");
    expect(screen.getByRole("checkbox", { name: CHANNEL.name })).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: incompatibleChannel.name }),
    ).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: "Create rule" }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.routing_tiers).toEqual([
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

  it("waits for guided defaults and returns to the setup workspace", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/models", async () => {
        await delay(300);
        return HttpResponse.json([MODEL]);
      }),
    );
    const user = userEvent.setup();
    renderAppAt(
      `/admin/routing/model-rules/new?upstreamModelId=${MODEL.id}&clientModel=${encodeURIComponent(
        MODEL.source_model_id,
      )}&returnTo=${encodeURIComponent("/admin/model-setup?view=rules")}`,
    );

    expect(
      await screen.findByRole("heading", { name: "New model rule" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("combobox", { name: "Client model" }),
    ).not.toBeInTheDocument();
    const clientModelSelect = await screen.findByRole("combobox", {
      name: "Client model",
    });
    expect(clientModelSelect).toHaveTextContent(MODEL.display_name);

    await user.click(screen.getByRole("button", { name: "Back to model setup" }));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/model-setup");
      expect(new URLSearchParams(window.location.search).get("view")).toBe(
        "rules",
      );
    });
  });
});
