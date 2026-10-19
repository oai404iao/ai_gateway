import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import type { RegistrationInvitationCodeUpdateInput } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { REGISTRATION_INVITATION_CODE, USER_GROUP } from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";

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

describe("RegistrationInvitationCodeDetailPage", () => {
  it("adjusts future registration settings with If-Match", async () => {
    seedAuthenticatedSession();
    let submitted: RegistrationInvitationCodeUpdateInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put(
        "/console/v1/registration-invitation-codes/:id",
        async ({ request }) => {
          submitted =
            (await request.json()) as RegistrationInvitationCodeUpdateInput;
          ifMatch = request.headers.get("If-Match");
          return HttpResponse.json({
            id: REGISTRATION_INVITATION_CODE.id,
            correlation_id: "00000000-0000-0000-0000-0000000000c4",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderAppAt(
      `/admin/registration-invitation-codes/${REGISTRATION_INVITATION_CODE.id}`,
    );

    const name = await screen.findByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Adjusted launch");

    await user.click(screen.getByRole("combobox"));
    await user.click(await screen.findByRole("option", { name: USER_GROUP.name }));

    const balance = screen.getByLabelText(/initial balance/i);
    await user.clear(balance);
    await user.type(balance, "30");
    const maximumUses = screen.getByLabelText("Maximum uses");
    await user.clear(maximumUses);
    await user.type(maximumUses, "150");
    fireEvent.change(screen.getByLabelText("Expires"), {
      target: { value: "2030-01-02T03:04" },
    });
    await user.click(screen.getByRole("switch", { name: "Enabled" }));
    await user.click(screen.getByRole("button", { name: "Save registration code" }));

    await waitFor(() => {
      expect(submitted).toEqual({
        name: "Adjusted launch",
        max_uses: 150,
        expires_at: new Date("2030-01-02T03:04").toISOString(),
        enabled: false,
        user_group_id: USER_GROUP.id,
        initial_balance_amount: "30",
      });
    });
    expect(ifMatch).toBe(`"${REGISTRATION_INVITATION_CODE.updated_at}"`);
  });
});
