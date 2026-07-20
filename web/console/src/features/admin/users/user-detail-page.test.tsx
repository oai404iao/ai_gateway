import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { API_KEY_POLICY, CONTROL_PLANE_USER } from "@/test/fixtures";
import type { UserInput } from "@/api/types";

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

describe("UserDetailPage", () => {
  it("submits loaded Select values without requiring reselection", async () => {
    seedAuthenticatedSession();
    const managedUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000099",
      email: "managed@example.test",
      display_name: "Managed Admin",
      status: "suspended",
    };
    let submitted: UserInput | undefined;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(managedUser, {
          headers: { ETag: `"${managedUser.updated_at}"` },
        }),
      ),
      http.put("/console/v1/users/:id", async ({ request }) => {
        submitted = (await request.json()) as UserInput;
        return HttpResponse.json({
          id: managedUser.id,
          correlation_id: "22222222-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/users/${managedUser.id}`);

    expect(await screen.findByDisplayValue(managedUser.display_name)).toBeInTheDocument();
    const balance = screen.getByDisplayValue(managedUser.balance_amount);
    await user.clear(balance);
    await user.type(balance, "25.5");
    await user.click(screen.getByRole("button", { name: /save user/i }));

    await waitFor(() => {
      expect(submitted).toEqual({
        display_name: managedUser.display_name,
        email: managedUser.email,
        role: "admin",
        status: "suspended",
        balance_amount: "25.5",
        currency: managedUser.currency,
        default_api_key_policy_id: API_KEY_POLICY.id,
      });
    });
  });
});
