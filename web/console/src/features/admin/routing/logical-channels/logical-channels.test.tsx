import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { LOGICAL_CHANNEL, ROUTING_GROUP, UPSTREAM_ACCESS } from "@/test/fixtures";
import type { LogicalChannelInput } from "@/api/types";

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

describe("logical channels", () => {
  it("lists channels with resolved group, access, and credential names", async () => {
    renderAt("/admin/routing/logical-channels");
    expect(await screen.findByText("Primary channel")).toBeInTheDocument();
    expect(screen.getAllByText(ROUTING_GROUP.name).length).toBeGreaterThan(0);
    expect(screen.getAllByText(UPSTREAM_ACCESS.name).length).toBeGreaterThan(0);
  });

  it("sends an explicit nullable credential binding on update", async () => {
    let submitted: LogicalChannelInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/routing/logical-channels/:id", async ({ request }) => {
        submitted = (await request.json()) as LogicalChannelInput;
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({ id: LOGICAL_CHANNEL.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`);
    await waitFor(() =>
      expect(screen.getByLabelText("Name")).toHaveValue(LOGICAL_CHANNEL.name),
    );
    await user.click(screen.getByRole("button", { name: "Save channel" }));
    await waitFor(() =>
      expect(submitted?.credential_id).toBe(LOGICAL_CHANNEL.credential_id),
    );
    expect(submitted?.group_id).toBe(LOGICAL_CHANNEL.group_id);
    expect(submitted?.access_id).toBe(LOGICAL_CHANNEL.access_id);
    expect(ifMatch).toBe(`"${LOGICAL_CHANNEL.updated_at}"`);
  });
});
