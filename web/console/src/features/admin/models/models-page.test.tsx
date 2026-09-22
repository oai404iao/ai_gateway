import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { MODEL, OPERATION_RULE, ROUTING_PROFILE } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";
import { http, HttpResponse } from "msw";
import type { OperationRuleInput } from "@/api/types";

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

describe("ModelsPage", () => {
  it("provides a direct pricing action without opening the general model editor", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/admin/models");

    await user.click(await screen.findByRole("button", { name: new RegExp(MODEL.display_name) }));
    await user.click(
      await screen.findByRole("button", { name: /configure pricing/i }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe(`/admin/models/${MODEL.id}/pricing`);
    });
    expect(
      await screen.findByText(/multiplier calculator/i),
    ).toBeInTheDocument();
  });

  it("edits an operation in the selected model's right-hand panel with If-Match", async () => {
    seedAuthenticatedSession();
    let input: OperationRuleInput | undefined;
    let ifMatch: string | null = null;
    server.use(http.put("/console/v1/routing/operation-rules/:id", async ({ request }) => {
      input = await request.json() as OperationRuleInput;
      ifMatch = request.headers.get("If-Match");
      return HttpResponse.json({ id: OPERATION_RULE.id });
    }));
    const user = userEvent.setup();
    renderAppAt(`/admin/models?model=${MODEL.id}&rule=${OPERATION_RULE.id}`);
    const panel = await screen.findByRole("region", { name: "Operation routing" });
    await user.click(await within(panel).findByRole("button", { name: "Save operation rule" }));
    await waitFor(() => expect(input?.model_routing_profile_id).toBe(ROUTING_PROFILE.id));
    expect(input?.operation).toBe(OPERATION_RULE.operation);
    expect(ifMatch).toBe(`"${OPERATION_RULE.updated_at}"`);
    expect(window.location.pathname).toBe("/admin/models");
  });

  it("creates the selected model's profile automatically before adding its first operation", async () => {
    seedAuthenticatedSession();
    let profileCreated = false;
    let created: OperationRuleInput | undefined;
    server.use(
      http.get("/console/v1/routing/profiles", () =>
        HttpResponse.json(profileCreated ? [ROUTING_PROFILE] : [])),
      http.get("/console/v1/routing/operation-rules", () =>
        HttpResponse.json(created ? [{ ...OPERATION_RULE, ...created }] : [])),
      http.post("/console/v1/routing/profiles", async ({ request }) => {
        expect(await request.json()).toEqual({ model_id: MODEL.id });
        profileCreated = true;
        return HttpResponse.json({ id: ROUTING_PROFILE.id }, { status: 201 });
      }),
      http.post("/console/v1/routing/operation-rules", async ({ request }) => {
        expect(profileCreated).toBe(true);
        created = await request.json() as OperationRuleInput;
        return HttpResponse.json({ id: OPERATION_RULE.id }, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/models?model=${MODEL.id}`);
    await user.click(await screen.findByRole("button", { name: "Add operation" }));
    await user.click(await screen.findByRole("button", { name: "Create operation rule" }));
    await waitFor(() => expect(created).toMatchObject({
      model_routing_profile_id: ROUTING_PROFILE.id,
      operation: "responses",
      enabled: false,
      routing_tiers: [],
    }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("rule")).toBe(OPERATION_RULE.id));
    expect(window.location.pathname).toBe("/admin/models");
  });

  it("offers only operations not already configured for the selected model", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt(`/admin/models?model=${MODEL.id}&rule=new`);
    await user.click(await screen.findByRole("combobox", { name: "Operation" }));
    expect(screen.queryByRole("option", { name: "Responses" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(4);
  });

  it("keeps a dirty operation selected when a concurrent creation conflicts", async () => {
    seedAuthenticatedSession();
    let saves = 0;
    server.use(
      http.get("/console/v1/routing/operation-rules", () =>
        HttpResponse.json(saves ? [OPERATION_RULE] : [])),
      http.post("/console/v1/routing/operation-rules", () => {
        saves += 1;
        return HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/models?model=${MODEL.id}&rule=new`);
    const enabled = await screen.findByRole("switch", { name: "Enabled" });
    await user.click(enabled);
    await user.click(enabled);
    await user.click(screen.getByRole("button", { name: "Create operation rule" }));
    expect(await screen.findByText("This rule was changed elsewhere. Reloading.")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "Create operation rule" })).toBeEnabled());
    expect(screen.getByRole("combobox", { name: "Operation" })).toHaveTextContent("Responses");
    await user.click(screen.getByRole("button", { name: "Create operation rule" }));
    expect(await screen.findByText("This operation is already configured. Choose another operation.")).toBeInTheDocument();
    expect(saves).toBe(1);
  });
});
