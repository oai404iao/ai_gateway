import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { USER_SETTINGS } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

function renderApp() {
  window.history.replaceState({}, "", "/account/settings");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("PersonalSettingsPage", () => {
  it("enables Responses WebSocket forwarding for the current user", async () => {
    seedAuthenticatedSession();
    let submitted: { websocket_enabled: boolean } | null = null;
    server.use(
      http.put("/console/v1/me/settings", async ({ request }) => {
        submitted = (await request.json()) as { websocket_enabled: boolean };
        return HttpResponse.json({
          ...USER_SETTINGS,
          websocket_enabled: submitted.websocket_enabled,
        });
      }),
    );
    const user = userEvent.setup();
    renderApp();

    const toggle = await screen.findByRole("switch", {
      name: "Enable Responses WebSocket",
    });
    expect(toggle).not.toBeChecked();

    await user.click(toggle);
    await user.click(
      screen.getByRole("button", { name: "Save personal settings" }),
    );

    await waitFor(() => expect(submitted).toEqual({ websocket_enabled: true }));
    expect(await screen.findByText("Personal settings saved.")).toBeInTheDocument();
  });
});
