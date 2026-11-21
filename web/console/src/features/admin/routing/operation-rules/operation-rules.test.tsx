import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { I18nProvider } from "@/app/i18n-provider";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { OPERATION_RULE, ROUTING_PROFILE, MODEL, CHANNEL_CAPABILITY, LOGICAL_CHANNEL } from "@/test/fixtures";
import { OperationRuleTierEditor } from "./operation-rule-tier-editor";
import type { OperationTierInput } from "@/api/types";

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
  it("orders the live allocation path by priority and edits only the selected tier", async () => {
    const candidate = OPERATION_RULE.routing_tiers[0].candidates[0];
    function Editor() {
      const [tiers, setTiers] = useState<OperationTierInput[]>([
        { ...OPERATION_RULE.routing_tiers[0], priority: 4 },
        { ...OPERATION_RULE.routing_tiers[0], priority: 0, candidates: [candidate, { ...candidate, upstream_model: "gpt-5-mini", weight: 3 }] },
      ]);
      return <OperationRuleTierEditor value={tiers} capabilities={[CHANNEL_CAPABILITY]} channelNames={new Map([[LOGICAL_CHANNEL.id, LOGICAL_CHANNEL.name]])} onChange={setTiers} errorFor={() => undefined} />;
    }
    const user = userEvent.setup();
    render(<I18nProvider><Editor /></I18nProvider>);
    const graph = within(screen.getByRole("region", { name: "Allocation path" }));
    expect(graph.getAllByRole("button")[0]).toHaveAccessibleName("Inspect tier 2");
    expect(graph.getByRole("button", { name: "Inspect tier 2" })).toHaveTextContent("75%");
    await user.click(graph.getByRole("button", { name: "Inspect tier 2" }));
    expect(screen.queryByLabelText("Weight for tier 1 row 1")).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Weight for tier 2 row 1"), { target: { value: "3" } });
    expect(graph.getAllByText("50%")).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Add routing tier" }));
    expect(graph.getByRole("button", { name: "Inspect tier 3" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Priority")).toHaveValue(1);
  });

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
