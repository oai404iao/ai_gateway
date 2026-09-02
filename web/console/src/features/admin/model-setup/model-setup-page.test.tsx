import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  CHANNEL,
  CHANNEL_GROUP,
  MODEL,
  MODEL_RULE,
} from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

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

describe("ModelSetupPage", () => {
  it("presents one guided endpoint-to-route workflow", async () => {
    seedAuthenticatedSession();
    renderAppAt("/admin/model-setup");

    expect(
      await screen.findByRole("heading", { name: "Model setup" }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText("One flow from endpoint to client model"),
    ).toBeInTheDocument();
    expect(screen.getByText("Supplier endpoint")).toBeInTheDocument();
    expect(screen.getByText("Model and pricing")).toBeInTheDocument();
    expect(screen.getByText("Published routing map")).toBeInTheDocument();
    expect(screen.getByText("gateway-chat-model")).toBeInTheDocument();
    expect(screen.getByText("GPT-4o mini")).toBeInTheDocument();
    expect(screen.getByText("P0 · chat-primary")).toBeInTheDocument();
  });

  it("copies an ordinary supplier without exposing or reusing its credential", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    let createdInput: Record<string, unknown> | null = null;
    server.use(
      http.post("/console/v1/routing/channels", async ({ request }) => {
        createdInput = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json(
          {
            id: "00000000-0000-0000-0000-000000000099",
            correlation_id: "00000000-0000-0000-0000-000000000098",
          },
          { status: 201 },
        );
      }),
    );
    renderAppAt("/admin/model-setup");

    const copySupplier = await screen.findByRole("button", {
      name: "Copy supplier",
    });
    await waitFor(() => expect(copySupplier).toBeEnabled());
    await user.click(copySupplier);
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: new RegExp(CHANNEL.name) }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/routing/channels/new");
    });
    expect(await screen.findByLabelText("Name")).toHaveValue(
      `${CHANNEL.name} copy`,
    );
    expect(screen.getByLabelText("Base URL")).toHaveValue(CHANNEL.base_url);
    expect(screen.getByLabelText("Upstream API key")).toHaveValue("");
    expect(new URLSearchParams(window.location.search).get("copyFrom")).toBe(
      CHANNEL.id,
    );
    await user.type(
      screen.getByLabelText("Upstream API key"),
      "sk-new-copy-credential",
    );
    await user.click(screen.getByRole("button", { name: "Create channel" }));

    await waitFor(() => {
      expect(createdInput).not.toBeNull();
      expect(window.location.pathname).toBe("/admin/model-setup");
    });
    expect(createdInput).toMatchObject({
      name: `${CHANNEL.name} copy`,
      upstream_api_key: "sk-new-copy-credential",
    });
    expect(JSON.stringify(createdInput)).not.toContain(
      "sk-upstream-test-secret",
    );
  });

  it("copies model pricing while requiring a new source model id", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    let createdInput: Record<string, unknown> | null = null;
    server.use(
      http.post("/console/v1/models", async ({ request }) => {
        createdInput = (await request.json()) as Record<string, unknown>;
        return HttpResponse.json(
          {
            id: "00000000-0000-0000-0000-000000000097",
            correlation_id: "00000000-0000-0000-0000-000000000096",
          },
          { status: 201 },
        );
      }),
    );
    renderAppAt("/admin/model-setup");

    const copyModel = await screen.findByRole("button", {
      name: "Copy model",
    });
    await waitFor(() => expect(copyModel).toBeEnabled());
    await user.click(copyModel);
    const dialog = await screen.findByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", {
        name: new RegExp(MODEL.display_name),
      }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/models/new");
    });
    expect(await screen.findByLabelText("Source model id")).toHaveValue("");
    expect(screen.getByLabelText("Display name")).toHaveValue(
      `${MODEL.display_name} copy`,
    );
    expect(screen.getByLabelText("Provider name")).toHaveValue(
      MODEL.provider_name,
    );
    expect(screen.getByLabelText(/Input unit price/)).toHaveValue(
      MODEL.input_unit_price,
    );
    await user.type(
      screen.getByLabelText("Source model id"),
      "openai/gpt-4.1-mini",
    );
    await user.click(
      screen.getByRole("button", { name: "Create copied model" }),
    );

    await waitFor(() => {
      expect(createdInput).not.toBeNull();
      expect(window.location.pathname).toBe("/admin/model-setup");
    });
    expect(createdInput).toMatchObject({
      source_model_id: "openai/gpt-4.1-mini",
      source_payload: {},
    });
  });

  it("marks only usable end-to-end configuration as ready", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([{ ...CHANNEL_GROUP, enabled: false }]),
      ),
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([
          { ...MODEL_RULE, routing_status: "disconnected" },
        ]),
      ),
    );
    renderAppAt("/admin/model-setup");

    expect(await screen.findByText("0 of 4 ready")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue setup" }),
    ).toBeEnabled();
    expect(screen.getAllByText("Needs setup")).toHaveLength(4);
  });

  it("does not complete publication with an unrelated ready rule", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([
          {
            ...MODEL_RULE,
            upstream_model_id:
              "00000000-0000-0000-0000-000000000088",
            upstream_model: "unrelated-model",
          },
        ]),
      ),
    );
    renderAppAt("/admin/model-setup");

    expect(await screen.findByText("3 of 4 ready")).toBeInTheDocument();
    expect(screen.getAllByText("Needs setup")).toHaveLength(1);
    expect(
      screen.getByRole("button", { name: "Continue setup" }),
    ).toBeEnabled();
  });

  it("does not count an unselected channel in a targeted group as published", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([
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
                    channels: [
                      {
                        channel_id:
                          "00000000-0000-0000-0000-000000000099",
                        weight: 100,
                      },
                    ],
                  },
                ],
              },
            ],
          },
        ]),
      ),
    );
    renderAppAt("/admin/model-setup");

    expect(await screen.findByText("3 of 4 ready")).toBeInTheDocument();
    expect(screen.getAllByText("Needs setup")).toHaveLength(1);
  });

  it("shows routing targets in lower-priority-first order", async () => {
    seedAuthenticatedSession();
    const fallbackGroup = {
      ...CHANNEL_GROUP,
      id: "00000000-0000-0000-0000-000000000098",
      name: "chat-fallback",
    };
    const fallbackChannel = {
      ...CHANNEL,
      id: "00000000-0000-0000-0000-000000000099",
      channel_group_id: fallbackGroup.id,
      name: "upstream-fallback",
    };
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([CHANNEL_GROUP, fallbackGroup]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([CHANNEL, fallbackChannel]),
      ),
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([
          {
            ...MODEL_RULE,
            routing_tiers: [
              {
                priority: 10,
                selection_strategy: "weighted_random",
                channel_groups: [
                  {
                    channel_group_id: fallbackGroup.id,
                    channel_selection: "all",
                    default_weight: 100,
                    channels: [],
                  },
                ],
              },
              {
                priority: 0,
                selection_strategy: "weighted_round_robin",
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
        ]),
      ),
    );
    renderAppAt("/admin/model-setup");

    const primary = await screen.findByText("P0 · upstream-a");
    const fallback = screen.getByText("P10 · chat-fallback");
    expect(
      primary.compareDocumentPosition(fallback) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("keeps the continuation action disabled until setup data is loaded", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/channel-groups", async () => {
        await delay(300);
        return HttpResponse.json([CHANNEL_GROUP]);
      }),
    );
    renderAppAt("/admin/model-setup");

    expect(
      await screen.findByRole("button", { name: "Continue setup" }),
    ).toBeDisabled();
    expect(
      await screen.findByRole("button", { name: "Review published routes" }),
    ).toBeEnabled();
  });

  it("does not expose a copy form until all prefill data is ready", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/channel-groups", async () => {
        await delay(300);
        return HttpResponse.json([CHANNEL_GROUP]);
      }),
    );
    renderAppAt(
      `/admin/routing/channels/new?copyFrom=${CHANNEL.id}&channelGroupId=${CHANNEL_GROUP.id}`,
    );

    expect(
      await screen.findByRole("heading", { name: "Copy supplier" }),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(await screen.findByLabelText("Name")).toHaveValue(
      `${CHANNEL.name} copy`,
    );
  });

  it("surfaces prefill dependency failures instead of showing empty defaults", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json(
          { error: "Groups unavailable" },
          { status: 500 },
        ),
      ),
    );
    renderAppAt(
      `/admin/routing/channels/new?copyFrom=${CHANNEL.id}&channelGroupId=${CHANNEL_GROUP.id}`,
    );

    expect(await screen.findByText("Request failed")).toBeInTheDocument();
    expect(screen.getByText(/Groups unavailable/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
  });

  it("keeps copied suppliers in a same-format channel group", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    const imageGroup = {
      ...CHANNEL_GROUP,
      id: "00000000-0000-0000-0000-000000000029",
      name: "images-primary",
      api_format: "open_ai_images" as const,
    };
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([CHANNEL_GROUP, imageGroup]),
      ),
    );
    renderAppAt(
      `/admin/routing/channels/new?copyFrom=${CHANNEL.id}&channelGroupId=${imageGroup.id}`,
    );

    expect(await screen.findByLabelText("Name")).toHaveValue(
      `${CHANNEL.name} copy`,
    );
    expect(screen.getByDisplayValue("Chat Completions")).toBeDisabled();
    expect(screen.queryByDisplayValue("Images")).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("combobox", { name: "Channel group" }),
    );
    const listbox = await screen.findByRole("listbox");
    expect(
      within(listbox).getByRole("option", { name: /chat-primary/ }),
    ).toBeInTheDocument();
    expect(
      within(listbox).queryByRole("option", { name: /images-primary/ }),
    ).not.toBeInTheDocument();
  });
});
