import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import type { RegisterInput } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { USER_ACCESS_TOKEN, USER_USER } from "@/test/fixtures";
import { server } from "@/test/msw";

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

describe("RegisterPage", () => {
  it("registers with an invitation code and signs the user in", async () => {
    let submitted: RegisterInput | undefined;
    server.use(
      http.post("/console/v1/auth/refresh", () =>
        HttpResponse.json({ error: "unauthorized" }, { status: 401 }),
      ),
      http.post("/console/v1/auth/register", async ({ request }) => {
        submitted = (await request.json()) as RegisterInput;
        return HttpResponse.json({
          access_token: USER_ACCESS_TOKEN,
          token_type: "Bearer",
          expires_in: 900,
          user: USER_USER,
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/register");

    await user.type(
      await screen.findByLabelText("Invitation code"),
      "COMMUNITY-ACCESS-2026",
    );
    await user.type(screen.getByLabelText("Email"), "new-user@example.test");
    await user.type(screen.getByLabelText("Display name"), "New User");
    await user.type(screen.getByLabelText("Password"), "long-enough-password");
    await user.type(screen.getByLabelText("Confirm password"), "long-enough-password");
    await user.click(screen.getByRole("button", { name: "Create account" }));

    await waitFor(() => {
      expect(submitted).toEqual({
        invitation_code: "COMMUNITY-ACCESS-2026",
        email: "new-user@example.test",
        display_name: "New User",
        password: "long-enough-password",
      });
    });
    await waitFor(() => expect(window.location.pathname).toBe("/account"));
  });
});
