import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type { ModelProtocolRuleCreateInput } from "@/api/types";
import {
  MODEL,
  MODEL_PROTOCOL_RULE,
  MODEL_RULE,
} from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";

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
  it("shows one priced client model with its protocol rules", async () => {
    seedAuthenticatedSession();
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    expect(
      await screen.findByRole("heading", { name: MODEL.display_name }),
    ).toBeInTheDocument();
    expect(screen.getAllByText(MODEL.source_model_id).length).toBeGreaterThan(
      0,
    );
    expect(
      screen.getByRole("link", { name: "Open Chat Completions" }),
    ).toBeInTheDocument();
    expect(screen.getByText("1 priority tiers · 1/1 active targets"))
      .toBeInTheDocument();
  });

  it("creates a disabled empty protocol draft under the parent rule", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    let submitted: ModelProtocolRuleCreateInput | undefined;
    server.use(
      http.post(
        "/console/v1/routing/model-rules/:id/protocols",
        async ({ request }) => {
          submitted = (await request.json()) as ModelProtocolRuleCreateInput;
          return HttpResponse.json({
            id: MODEL_PROTOCOL_RULE.id,
            correlation_id: "66666666-0000-0000-0000-000000000000",
          });
        },
      ),
    );
    renderAppAt(`/admin/routing/model-rules/${MODEL_RULE.id}`);

    const responsesCard = (await screen.findByText("Responses")).closest(
      '[data-slot="card"]',
    );
    expect(responsesCard).not.toBeNull();
    await user.click(
      screen.getAllByRole("button", { name: "Add protocol" })[0],
    );

    await waitFor(() => {
      expect(submitted).toEqual({ api_format: "open_ai_responses" });
    });
  });
});
