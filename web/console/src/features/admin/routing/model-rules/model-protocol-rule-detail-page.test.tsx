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
import { CHANNEL, MODEL_PROTOCOL_RULE, MODEL_RULE } from "@/test/fixtures";
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

function routeProtocol(
  candidates: ModelProtocolRuleInput["routing_tiers"][number]["candidates"],
): ModelProtocolRuleView {
  return {
    ...MODEL_PROTOCOL_RULE,
    routing_tiers: [
      {
        priority: 0,
        selection_strategy: "weighted_random",
        candidates,
      },
    ],
    target_candidate_count: candidates.length,
    model_capable_candidate_count: candidates.length,
    active_candidate_count: candidates.length,
  };
}

describe("ModelProtocolRuleDetailPage", () => {
  it("blocks incomplete records and invalid weights before making a save request", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    let saveCount = 0;
    server.use(
      http.put(
        "/console/v1/routing/model-rules/:id/protocols/:protocolId",
        () => {
          saveCount += 1;
          return HttpResponse.json({});
        },
      ),
    );
    renderPage();
    await user.click(await screen.findByRole("button", { name: "Add record" }));
    const weight = screen.getByRole("spinbutton", {
      name: "Weight for tier 1 row 2",
    });
    await user.clear(weight);
    await user.type(weight, "0");
    await user.click(screen.getByRole("button", { name: "Save protocol" }));
    expect(await screen.findByText("Choose a channel.")).toBeInTheDocument();
    expect(screen.getByText("Choose an upstream model.")).toBeInTheDocument();
    expect(
      screen.getByText("Weight must be a positive integer."),
    ).toBeInTheDocument();
    expect(saveCount).toBe(0);
  });

  it("adds an editable blank record and saves the selected channel, model and weight", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    const secondChannel = {
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
    const protocol = routeProtocol([
      {
        channel_id: CHANNEL.id,
        upstream_model: "wire-a",
        weight: 100,
      },
    ]);
    let submitted: ModelProtocolRuleInput | undefined;
    server.use(
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([
          { ...CHANNEL, available_models: ["wire-a", "wire-a-2"] },
          secondChannel,
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

    await user.click(await screen.findByRole("button", { name: "Add record" }));
    expect(
      screen.queryByRole("combobox", {
        name: "Bulk-add channel group to tier 1",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }),
    ).toBeDisabled();
    await user.click(
      screen.getByRole("combobox", { name: "Channel for tier 1 row 2" }),
    );
    expect(
      within(await screen.findByRole("listbox")).getByRole("option", {
        name: /no-models/,
      }),
    ).toHaveAttribute("aria-disabled", "true");
    await user.click(
      within(await screen.findByRole("listbox")).getByRole("option", {
        name: /wire-b-only/,
      }),
    );
    await user.click(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }),
    );
    await user.click(await screen.findByRole("option", { name: "wire-b" }));
    const weight = screen.getByRole("spinbutton", {
      name: "Weight for tier 1 row 2",
    });
    expect(weight).toHaveValue(1);
    await user.clear(weight);
    await user.type(weight, "3");
    await user.click(screen.getByRole("button", { name: "Save protocol" }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.routing_tiers[0].candidates).toEqual([
      {
        channel_id: CHANNEL.id,
        upstream_model: "wire-a",
        weight: 100,
      },
      {
        channel_id: secondChannel.id,
        upstream_model: "wire-b",
        weight: 3,
      },
    ]);
    expect(JSON.stringify(submitted)).not.toContain("channel_group_id");
    expect(JSON.stringify(submitted)).not.toContain(channelWithoutModels.id);
  });

  it("serializes multiple upstream models for the same channel", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    const protocol = routeProtocol([
      {
        channel_id: CHANNEL.id,
        upstream_model: "wire-a",
        weight: 100,
      },
    ]);
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
            correlation_id: "77777777-0000-0000-0000-000000000001",
          });
        },
      ),
    );
    renderPage();

    await user.click(await screen.findByRole("button", { name: "Add record" }));
    await user.click(
      screen.getByRole("combobox", { name: "Channel for tier 1 row 2" }),
    );
    await user.click(
      within(await screen.findByRole("listbox")).getByRole("option", {
        name: new RegExp(CHANNEL.name),
      }),
    );
    await user.click(
      screen.getByRole("combobox", { name: "Upstream model for tier 1 row 2" }),
    );
    expect(
      await screen.findByRole("option", { name: "wire-a" }),
    ).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("option", { name: "wire-b" }));
    await user.click(screen.getByRole("button", { name: "Save protocol" }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.routing_tiers[0].candidates).toEqual([
      {
        channel_id: CHANNEL.id,
        upstream_model: "wire-a",
        weight: 100,
      },
      {
        channel_id: CHANNEL.id,
        upstream_model: "wire-b",
        weight: 1,
      },
    ]);
  });

  it("keeps an empty protocol as a disabled draft", async () => {
    seedAuthenticatedSession();
    const draft: ModelProtocolRuleView = {
      ...MODEL_PROTOCOL_RULE,
      routing_tiers: [],
      enabled: false,
      routing_status: "draft",
      target_candidate_count: 0,
      model_capable_candidate_count: 0,
      active_candidate_count: 0,
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
            correlation_id: "77777777-0000-0000-0000-000000000002",
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
