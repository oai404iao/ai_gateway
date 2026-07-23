import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL, CHANNEL_GROUP, MODEL } from "@/test/fixtures";
import type {
  ChannelGroupView,
  ChannelView,
  ControlPlaneModel,
  ModelRuleInput,
} from "@/api/types";

const SECOND_MODEL: ControlPlaneModel = {
  ...MODEL,
  id: "00000000-0000-0000-0000-000000000031",
  source_model_id: "anthropic/claude-sonnet",
  display_name: "Claude Sonnet",
  provider_name: "Anthropic",
};

const RESPONSES_GROUP: ChannelGroupView = {
  ...CHANNEL_GROUP,
  id: "00000000-0000-0000-0000-000000000023",
  name: "responses-primary",
  api_format: "open_ai_responses",
};

const CHAT_CHANNEL: ChannelView = {
  ...CHANNEL,
  available_models: [MODEL.source_model_id, SECOND_MODEL.source_model_id],
};

const RESPONSES_CHANNEL: ChannelView = {
  ...CHANNEL,
  id: "00000000-0000-0000-0000-000000000024",
  channel_group_id: RESPONSES_GROUP.id,
  api_format: RESPONSES_GROUP.api_format,
  name: "responses-upstream",
  available_models: [MODEL.source_model_id, SECOND_MODEL.source_model_id],
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

describe("ModelRuleQuickAddDialog", () => {
  it("creates rules for multiple models across every compatible API format", async () => {
    seedAuthenticatedSession();
    const submitted: ModelRuleInput[] = [];
    server.use(
      http.get("/console/v1/models", () => HttpResponse.json([MODEL, SECOND_MODEL])),
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([CHANNEL_GROUP, RESPONSES_GROUP]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([CHAT_CHANNEL, RESPONSES_CHANNEL]),
      ),
      http.get("/console/v1/routing/model-rules", () => HttpResponse.json([])),
      http.post("/console/v1/routing/model-rules", async ({ request }) => {
        submitted.push((await request.json()) as ModelRuleInput);
        return HttpResponse.json(
          {
            id: `00000000-0000-0000-0000-${String(submitted.length).padStart(12, "0")}`,
            correlation_id: "77777777-0000-0000-0000-000000000000",
          },
          { status: 201 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/routing/model-rules");

    await user.click(await screen.findByRole("button", { name: "Quick add" }));
    await user.click(
      await screen.findByRole("checkbox", { name: `Select ${MODEL.source_model_id}` }),
    );
    await user.click(
      screen.getByRole("checkbox", { name: `Select ${SECOND_MODEL.source_model_id}` }),
    );
    await user.click(screen.getByRole("button", { name: "Create 4 rules" }));

    await waitFor(() => {
      expect(submitted).toHaveLength(4);
    });

    for (const model of [MODEL, SECOND_MODEL]) {
      expect(submitted).toContainEqual({
        client_model: model.source_model_id,
        api_format: "open_ai_chat_completions",
        upstream_model_id: model.id,
        description: null,
        channel_group_ids: [CHANNEL_GROUP.id],
        channel_ids: [],
        enabled: true,
      });
      expect(submitted).toContainEqual({
        client_model: model.source_model_id,
        api_format: "open_ai_responses",
        upstream_model_id: model.id,
        description: null,
        channel_group_ids: [RESPONSES_GROUP.id],
        channel_ids: [],
        enabled: true,
      });
    }
  });
});
