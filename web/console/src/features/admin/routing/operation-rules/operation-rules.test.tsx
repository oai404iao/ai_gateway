import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL_CAPABILITY, OPERATION_RULE } from "@/test/fixtures";
import type { OperationRuleInput } from "@/api/types";

function renderAt(path: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("operation rules", () => {
  it("lists rules with their capability/model tiers", async () => {
    renderAt("/admin/routing/operation-rules");
    expect(await screen.findByText(OPERATION_RULE.model_routing_profile_id)).toBeInTheDocument();
    expect(screen.getAllByText("Responses").length).toBeGreaterThan(0);
  });

  it("updates the tier graph with capability candidates and If-Match", async () => {
    let submitted: OperationRuleInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/routing/operation-rules/:id", async ({ request }) => {
        submitted = (await request.json()) as OperationRuleInput;
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({ id: OPERATION_RULE.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/operation-rules/${OPERATION_RULE.id}`);
    await waitFor(() =>
      expect(screen.getByLabelText("Model routing profile")).toHaveValue(
        OPERATION_RULE.model_routing_profile_id,
      ),
    );
    await user.click(screen.getByRole("button", { name: "Save operation rule" }));
    await waitFor(() =>
      expect(submitted?.routing_tiers[0].candidates[0].capability_id).toBe(
        CHANNEL_CAPABILITY.id,
      ),
    );
    expect(submitted?.operation).toBe("responses");
    expect(ifMatch).toBe(`"${OPERATION_RULE.updated_at}"`);
  });
});
