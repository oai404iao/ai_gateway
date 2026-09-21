import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL } from "@/test/fixtures";
import type { ChannelCapabilityInput } from "@/api/types";

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

describe("channel capabilities", () => {
  it("lists capabilities with their operation and transports", async () => {
    renderAt("/admin/routing/capabilities");
    expect(await screen.findByText("Primary channel")).toBeInTheDocument();
    expect(screen.getAllByText("Responses").length).toBeGreaterThan(0);
    expect(screen.getAllByText("HTTP JSON, HTTP SSE").length).toBeGreaterThan(0);
  });

  it("preserves operation settings and sends If-Match on update", async () => {
    let submitted: ChannelCapabilityInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/routing/capabilities/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelCapabilityInput;
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({ id: CHANNEL_CAPABILITY.id });
      }),
    );
    const user = userEvent.setup();
    renderAt(`/admin/routing/capabilities/${CHANNEL_CAPABILITY.id}`);
    await waitFor(() =>
      expect(screen.getByLabelText("Logical channel")).toHaveTextContent(
        LOGICAL_CHANNEL.name,
      ),
    );
    await user.click(screen.getByRole("button", { name: "Save capability" }));
    await waitFor(() =>
      expect(submitted?.settings.available_models).toEqual(["gpt-5", "gpt-5-mini"]),
    );
    expect(submitted?.channel_id).toBe(LOGICAL_CHANNEL.id);
    expect(submitted?.settings.operation).toBe("responses");
    expect(submitted?.settings.transports).toEqual(["http_json", "http_sse"]);
    expect(ifMatch).toBe(`"${CHANNEL_CAPABILITY.updated_at}"`);
  });
});
