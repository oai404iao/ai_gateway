import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedPasswordChangeSession } from "@/test/msw";
import {
  TEMPORARY_PASSWORD_USER,
  USER_ACCESS_TOKEN,
} from "@/test/fixtures";

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

describe("CompletePasswordResetPage", () => {
  it("redirects a restricted session and replaces the temporary password", async () => {
    seedPasswordChangeSession();
    let submitted: unknown;
    server.use(
      http.post(
        "/console/v1/auth/complete-password-reset",
        async ({ request }) => {
          submitted = await request.json();
          return HttpResponse.json({
            access_token: USER_ACCESS_TOKEN,
            token_type: "Bearer",
            expires_in: 900,
            user: {
              ...TEMPORARY_PASSWORD_USER,
              password_change_required: false,
              temporary_password_expires_at: null,
            },
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/statistics");

    expect(await screen.findByText("Set a new password")).toBeInTheDocument();
    expect(window.location.pathname).toBe("/change-password");
    await user.type(screen.getByLabelText("New password"), "new-permanent-password");
    await user.type(
      screen.getByLabelText("Confirm new password"),
      "new-permanent-password",
    );
    await user.click(screen.getByRole("button", { name: "Save new password" }));

    await waitFor(() => {
      expect(submitted).toEqual({ new_password: "new-permanent-password" });
      expect(window.location.pathname).toBe("/account");
    });
  });
});
