import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
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

    expect(
      await screen.findByRole("button", { name: /console operator/i }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Profile updated")).toBeInTheDocument();
  });

  it("clears the local session after a successful password change", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/account");

    await user.type(await screen.findByLabelText("Current password"), "current-password-123");
    await user.type(screen.getByLabelText("New password"), "replacement-password-123");
    await user.type(screen.getByLabelText("Confirm new password"), "replacement-password-123");
    await user.click(screen.getByRole("button", { name: /change password/i }));

    expect(await screen.findByLabelText("Email")).toBeInTheDocument();
  });
});
