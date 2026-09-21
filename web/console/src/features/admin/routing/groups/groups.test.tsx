import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { ROUTING_GROUP } from "@/test/fixtures";
import type { RoutingGroupInput } from "@/api/types";

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

describe("routing groups", () => {
  it("lists canonical groups with their sharing switch", async () => {
    renderAt("/admin/routing/groups");
    expect(await screen.findByText("Primary group")).toBeInTheDocument();
  });

  it("creates an enabled group", async () => {
    let created: RoutingGroupInput | undefined;
    server.use(
      http.post("/console/v1/routing/groups", async ({ request }) => {
        created = (await request.json()) as RoutingGroupInput;
        return HttpResponse.json({ id: ROUTING_GROUP.id }, { status: 201 });
      }),
    );
    const user = userEvent.setup();
    renderAt("/admin/routing/groups/new");
    await user.type(await screen.findByLabelText("Name"), "Fresh group");
    await user.click(screen.getByRole("button", { name: "Save group" }));
    await waitFor(() =>
      expect(created).toEqual({ name: "Fresh group", enabled: true, sharing_only: false }),
    );
  });

  it("edits an existing group with If-Match", async () => {
    let updated: RoutingGroupInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/routing/groups/:id", async ({ request }) => {
        updated = (await request.json()) as RoutingGroupInput;
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({ id: ROUTING_GROUP.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/groups/${ROUTING_GROUP.id}`);
    await waitFor(() =>
      expect(screen.getByLabelText("Name")).toHaveValue(ROUTING_GROUP.name),
    );
    await user.click(screen.getByRole("button", { name: "Save group" }));
    await waitFor(() => expect(updated?.name).toBe(ROUTING_GROUP.name));
    expect(ifMatch).toBe(`"${ROUTING_GROUP.updated_at}"`);
  });
});
