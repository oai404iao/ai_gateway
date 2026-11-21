import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { ADMIN_PROFILE } from "@/test/fixtures";

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

describe("ProfilePage", () => {
  it("preserves an unsaved WebSocket preference until the account tab change is confirmed", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/account/settings");

    const toggle = await screen.findByRole("switch", { name: "Enable Responses WebSocket" });
    await user.click(toggle);
    const nav = within(screen.getByRole("navigation", { name: "Account settings" }));
    expect(nav.getByRole("link", { name: "Personal settings" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await user.click(nav.getByRole("link", { name: "Profile" }));
    const confirmation = within(await screen.findByRole("alertdialog"));
    await user.click(confirmation.getByRole("button", { name: "Cancel" }));
    expect(toggle).toBeChecked();
    await user.click(nav.getByRole("link", { name: "Profile" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Discard changes",
      }),
    );
    expect(await screen.findByLabelText("Display name")).toBeVisible();
    expect(window.location.pathname).toBe("/account");
  });

  it("updates the shell identity after saving the display name", async () => {
    seedAuthenticatedSession();
    server.use(
      http.patch("/console/v1/me", async ({ request }) => {
        const body = (await request.json()) as { display_name: string };
        return HttpResponse.json({
          ...ADMIN_PROFILE,
          display_name: body.display_name,
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/account");

    const displayName = await screen.findByLabelText("Display name");
    await user.clear(displayName);
    await user.type(displayName, "Console Operator");
    await user.click(screen.getByRole("button", { name: /save display name/i }));

    expect(await screen.findByText("Profile updated")).toBeInTheDocument();
    const accountMenu = screen.getByRole("button", { name: "Account menu" });
    expect(accountMenu).toHaveAttribute("title", "Console Operator");
    await user.click(accountMenu);
    expect(within(await screen.findByRole("menu")).getByText("Console Operator")).toBeVisible();
  });

  it("clears the local session after a successful password change", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/account");

    await user.click(await screen.findByRole("button", { name: "Change password" }));
    const dialog = within(await screen.findByRole("dialog", { name: "Change password" }));
    await user.type(dialog.getByLabelText("Current password"), "current-password-123");
    await user.type(dialog.getByLabelText("New password"), "replacement-password-123");
    await user.type(dialog.getByLabelText("Confirm new password"), "replacement-password-123");
    await user.click(dialog.getByRole("button", { name: "Change password" }));

    expect(await screen.findByLabelText("Email")).toBeInTheDocument();
  });
});
