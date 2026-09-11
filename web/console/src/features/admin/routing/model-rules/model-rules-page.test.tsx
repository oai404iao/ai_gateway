import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import type {
  ControlPlaneModel,
  ModelRuleCreateInput,
} from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  IMAGE_MODEL_RULE,
  MODEL,
  MODEL_RULE,
} from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

const AVAILABLE_MODEL: ControlPlaneModel = {
  ...MODEL,
  id: "00000000-0000-0000-0000-000000000031",
  source_model_id: "openai/gpt-4.1",
  display_name: "GPT-4.1",
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
  it("lists one parent per priced model with its protocols together", async () => {
    seedAuthenticatedSession();
    const rule = {
      ...MODEL_RULE,
      protocol_rules: [
        ...MODEL_RULE.protocol_rules,
        ...IMAGE_MODEL_RULE.protocol_rules.map((protocol) => ({
          ...protocol,
          model_rule_id: MODEL_RULE.id,
        })),
      ],
    };
    server.use(
      http.get("/console/v1/models", () =>
        HttpResponse.json([MODEL, AVAILABLE_MODEL]),
      ),
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([rule]),
      ),
    );
    renderPage();

    const row = (await screen.findByText(MODEL.display_name)).closest("tr");
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).getByText("Chat Completions"))
      .toBeInTheDocument();
    expect(within(row as HTMLElement).getByText("Images")).toBeInTheDocument();
  });

  it("offers only unassigned enabled pricing models when creating", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    let submitted: ModelRuleCreateInput | undefined;
    server.use(
      http.get("/console/v1/models", () =>
        HttpResponse.json([MODEL, AVAILABLE_MODEL]),
      ),
      http.post("/console/v1/routing/model-rules", async ({ request }) => {
        submitted = (await request.json()) as ModelRuleCreateInput;
        return HttpResponse.json({
          id: "00000000-0000-0000-0000-000000000099",
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: "New model rule" }),
    );
    await user.click(screen.getByRole("combobox", { name: "Priced client model" }));
    const listbox = await screen.findByRole("listbox");
    expect(
      within(listbox).getByRole("option", {
        name: `${AVAILABLE_MODEL.display_name} (${AVAILABLE_MODEL.source_model_id})`,
      }),
    ).toBeInTheDocument();
    expect(
      within(listbox).queryByRole("option", {
        name: `${MODEL.display_name} (${MODEL.source_model_id})`,
      }),
    ).not.toBeInTheDocument();
    await user.click(
      within(listbox).getByRole("option", {
        name: `${AVAILABLE_MODEL.display_name} (${AVAILABLE_MODEL.source_model_id})`,
      }),
    );
    await user.click(screen.getByRole("button", { name: "Create rule" }));

    await waitFor(() => {
      expect(submitted).toEqual({ model_id: AVAILABLE_MODEL.id });
    });
  });
});
