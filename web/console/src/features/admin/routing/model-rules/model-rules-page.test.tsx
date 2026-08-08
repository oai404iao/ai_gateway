import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import type { ControlPlaneModel, ModelRuleView } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { MODEL, MODEL_RULE } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

const RESPONSES_RULE: ModelRuleView = {
  ...MODEL_RULE,
  id: "00000000-0000-0000-0000-000000000026",
  client_model: "gateway-responses-model",
  api_format: "open_ai_responses",
};

const SECOND_OPENAI_MODEL: ControlPlaneModel = {
  ...MODEL,
  id: "00000000-0000-0000-0000-000000000031",
  source_model_id: "openai/gpt-4.1",
  display_name: "GPT-4.1",
};

const SECOND_OPENAI_RULE: ModelRuleView = {
  ...MODEL_RULE,
  id: "00000000-0000-0000-0000-000000000032",
  client_model: SECOND_OPENAI_MODEL.source_model_id,
  upstream_model_id: SECOND_OPENAI_MODEL.id,
  upstream_model: SECOND_OPENAI_MODEL.source_model_id,
};

const ANTHROPIC_MODEL: ControlPlaneModel = {
  ...MODEL,
  id: "00000000-0000-0000-0000-000000000033",
  source_model_id: "anthropic/claude-sonnet",
  display_name: "Claude Sonnet",
  provider_name: "Anthropic",
};

const ANTHROPIC_RULE: ModelRuleView = {
  ...MODEL_RULE,
  id: "00000000-0000-0000-0000-000000000034",
  client_model: ANTHROPIC_MODEL.source_model_id,
  upstream_model_id: ANTHROPIC_MODEL.id,
  upstream_model: ANTHROPIC_MODEL.source_model_id,
};

function renderPage() {
  window.history.replaceState({}, "", "/admin/routing/model-rules");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("ModelRulesPage", () => {
  it("groups rules by provider and model id with formats kept together", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/models", () =>
        HttpResponse.json([SECOND_OPENAI_MODEL, ANTHROPIC_MODEL, MODEL]),
      ),
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([
          RESPONSES_RULE,
          ANTHROPIC_RULE,
          SECOND_OPENAI_RULE,
          MODEL_RULE,
        ]),
      ),
    );
    renderPage();

    const sharedModelLabel = await screen.findByText(
      `${MODEL.provider_name} · ${MODEL.source_model_id}`,
    );
    const sharedModelRow = sharedModelLabel.closest("tr");
    expect(sharedModelRow).not.toBeNull();
    expect(within(sharedModelRow as HTMLElement).getByText("2")).toBeInTheDocument();
    expect(sharedModelRow?.nextElementSibling).toHaveTextContent(
      `${MODEL_RULE.client_model}Chat Completions`,
    );
    expect(sharedModelRow?.nextElementSibling?.nextElementSibling).toHaveTextContent(
      `${RESPONSES_RULE.client_model}Responses`,
    );

    const groupedRows = screen
      .getAllByRole("cell")
      .filter((cell) => cell.hasAttribute("colspan"));
    expect(groupedRows.map((cell) => cell.textContent)).toEqual([
      `Anthropic · ${ANTHROPIC_MODEL.source_model_id}1`,
      `OpenAI · ${SECOND_OPENAI_MODEL.source_model_id}1`,
      `OpenAI · ${MODEL.source_model_id}2`,
    ]);
  });
});
