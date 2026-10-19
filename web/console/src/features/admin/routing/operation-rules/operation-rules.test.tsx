import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { OPERATION_RULE, ROUTING_PROFILE, MODEL } from "@/test/fixtures";

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
  it("creates an explicit priced-model profile before creating a draft operation rule", async () => {
    seedAuthenticatedSession();
    let profileCreated = false;
    let profileInput: unknown;
    let ruleInput: unknown;
    server.use(
      http.get("/console/v1/routing/profiles", () =>
        HttpResponse.json(profileCreated ? [ROUTING_PROFILE] : [])),
      http.post("/console/v1/routing/profiles", async ({ request }) => {
        profileInput = await request.json();
        profileCreated = true;
        return HttpResponse.json({ id: ROUTING_PROFILE.id }, { status: 201 });
      }),
      http.post("/console/v1/routing/operation-rules", async ({ request }) => {
        ruleInput = await request.json();
        return HttpResponse.json({ id: OPERATION_RULE.id }, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAt("/admin/routing/operation-rules/new");
    await user.click(await screen.findByLabelText("Create profile for pricing model"));
    await user.click(screen.getByRole("option", { name: `${MODEL.display_name} (${MODEL.source_model_id})` }));
    await user.click(screen.getByRole("button", { name: "Create routing profile" }));
    await waitFor(() => expect(screen.getByLabelText("Model routing profile")).toHaveTextContent(MODEL.display_name));
    expect(profileInput).toEqual({ model_id: MODEL.id });
    expect(ruleInput).toBeUndefined();
    await user.click(screen.getByRole("button", { name: "Create operation rule" }));
    await waitFor(() => expect(ruleInput).toEqual({
      model_routing_profile_id: ROUTING_PROFILE.id, operation: "responses",
      enabled: false, routing_tiers: [],
    }));
  });
});
