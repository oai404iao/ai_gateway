import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import type {
  ModelProtocolRuleInput,
  ModelProtocolRuleView,
} from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  CHANNEL,
  CHANNEL_GROUP,
  MODEL_PROTOCOL_RULE,
  MODEL_RULE,
} from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

function renderPage() {
  window.history.replaceState(
    {},
    "",
    `/admin/routing/model-rules/${MODEL_RULE.id}/protocols/${MODEL_PROTOCOL_RULE.id}`,
  );
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("ModelProtocolRuleDetailPage", () => {
  it("expands an all target only to channels advertising its model", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    const incompatibleChannel = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000199",
      name: "wire-b-only",
      available_models: ["wire-b"],
    };
    const channelWithoutModels = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000198",
      name: "no-models",
      available_models: [],
    };
    const protocol: ModelProtocolRuleView = {
      ...MODEL_PROTOCOL_RULE,
      routing_tiers: [
        {
          priority: 0,
          selection_strategy: "weighted_random",
          channel_groups: [
            {
              channel_group_id: CHANNEL.channel_group_id,
              channel_selection: "all",
              upstream_model: "wire-a",
              default_weight: 100,
              channels: [],
            },
          ],
        },
      ],
    };
    server.use(
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([
          { ...CHANNEL, available_models: ["wire-a"] },
          incompatibleChannel,
          channelWithoutModels,
        ]),
      ),
      http.get(
        "/console/v1/routing/model-rules/:id/protocols/:protocolId",
        () =>
          HttpResponse.json(protocol, {
            headers: { ETag: `"${protocol.updated_at}"` },
          }),
      ),
    );
    renderPage();

    expect(
      await screen.findByRole("checkbox", {
        name: incompatibleChannel.name,
      }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.getByRole("checkbox", { name: channelWithoutModels.name }),
    ).toHaveAttribute("aria-disabled", "true");
    await user.click(
      screen.getByRole("combobox", {
        name: `Channel selection for ${CHANNEL_GROUP.name}`,
      }),
    );
    await user.click(
      within(await screen.findByRole("listbox")).getByRole("option", {
        name: "Selected channels",
      }),
    );

    expect(screen.getByRole("checkbox", { name: CHANNEL.name })).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: incompatibleChannel.name }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: incompatibleChannel.name }),
    ).not.toHaveAttribute("aria-disabled");
    expect(
      screen.getByRole("checkbox", { name: channelWithoutModels.name }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.getByRole("combobox", {
        name: `Upstream model for channel ${CHANNEL.name}`,
      }),
    ).toHaveTextContent("wire-a");
  });

  it("selects and serializes the upstream model owned by a channel target", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    const protocol: ModelProtocolRuleView = {
      ...MODEL_PROTOCOL_RULE,
      routing_tiers: [
        {
          priority: 0,
          selection_strategy: "weighted_random",
          channel_groups: [
            {
              channel_group_id: CHANNEL.channel_group_id,
              channel_selection: "selected",
              upstream_model: null,
              default_weight: null,
              channels: [
                {
                  channel_id: CHANNEL.id,
                  upstream_model: "wire-a",
                  weight: 100,
                },
              ],
            },
          ],
        },
      ],
    };
    let submitted: ModelProtocolRuleInput | undefined;
    server.use(
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([
          {
            ...CHANNEL,
            available_models: ["wire-a", "wire-b"],
          },
        ]),
      ),
      http.get(
        "/console/v1/routing/model-rules/:id/protocols/:protocolId",
        () =>
          HttpResponse.json(protocol, {
            headers: { ETag: `"${protocol.updated_at}"` },
          }),
      ),
      http.put(
        "/console/v1/routing/model-rules/:id/protocols/:protocolId",
        async ({ request }) => {
          submitted = (await request.json()) as ModelProtocolRuleInput;
          return HttpResponse.json({
            id: protocol.id,
            correlation_id: "77777777-0000-0000-0000-000000000000",
          });
        },
      ),
    );
    renderPage();

    const modelSelect = await screen.findByRole("combobox", {
      name: `Upstream model for channel ${CHANNEL.name}`,
    });
    await user.click(modelSelect);
    const listbox = await screen.findByRole("listbox");
    expect(within(listbox).queryByRole("textbox")).not.toBeInTheDocument();
    await user.click(within(listbox).getByRole("option", { name: "wire-b" }));
    await user.click(screen.getByRole("button", { name: "Save protocol" }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(
      submitted?.routing_tiers[0].channel_groups[0].channels[0]
        .upstream_model,
    ).toBe("wire-b");
  });

  it("keeps an empty protocol as a disabled draft", async () => {
    seedAuthenticatedSession();
    const draft: ModelProtocolRuleView = {
      ...MODEL_PROTOCOL_RULE,
      routing_tiers: [],
      enabled: false,
      routing_status: "draft",
      target_channel_count: 0,
      model_capable_channel_count: 0,
      active_channel_count: 0,
    };
    let submitted: ModelProtocolRuleInput | undefined;
    server.use(
      http.get(
        "/console/v1/routing/model-rules/:id/protocols/:protocolId",
        () =>
          HttpResponse.json(draft, {
            headers: { ETag: `"${draft.updated_at}"` },
          }),
      ),
      http.put(
        "/console/v1/routing/model-rules/:id/protocols/:protocolId",
        async ({ request }) => {
          submitted = (await request.json()) as ModelProtocolRuleInput;
          return HttpResponse.json({
            id: draft.id,
            correlation_id: "77777777-0000-0000-0000-000000000001",
          });
        },
      ),
    );
    renderPage();

    await screen.findByText("Draft");
    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Save protocol" }));
    await waitFor(() => {
      expect(submitted).toEqual({
        description: null,
        routing_tiers: [],
        enabled: false,
      });
    });
  });
});
