import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import type {
  RegistrationInvitationCodeCreateInput,
  RegistrationInvitationCodeUpdateInput,
} from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  DEFAULT_USER_GROUP,
  REGISTRATION_INVITATION_CODE,
  USER_GROUP,
} from "@/test/fixtures";
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
  it("creates a reusable code with the default user group selected", async () => {
    seedAuthenticatedSession();
    let submitted: RegistrationInvitationCodeCreateInput | undefined;
    server.use(
      http.post(
        "/console/v1/registration-invitation-codes",
        async ({ request }) => {
          submitted =
            (await request.json()) as RegistrationInvitationCodeCreateInput;
          return HttpResponse.json(
            {
              id: REGISTRATION_INVITATION_CODE.id,
              invitation_code: "COMMUNITY-ACCESS-2026",
              correlation_id: "00000000-0000-0000-0000-0000000000c3",
            },
            { status: 201 },
          );
        },
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/registration-invitation-codes/new");

    await user.type(await screen.findByLabelText("Name"), "Community launch");
    await user.type(screen.getByLabelText("Invitation code"), "COMMUNITY-ACCESS-2026");
    const balance = screen.getByLabelText(/initial balance/i);
    await user.clear(balance);
    await user.type(balance, "25");
    const maximumUses = screen.getByLabelText("Maximum uses");
    await user.type(maximumUses, "100");
    await user.click(
      screen.getByRole("button", { name: "Create registration code" }),
    );

    await waitFor(() => {
      expect(submitted).toEqual({
        name: "Community launch",
        invitation_code: "COMMUNITY-ACCESS-2026",
        max_uses: 100,
        expires_at: null,
        enabled: true,
        user_group_id: DEFAULT_USER_GROUP.id,
        initial_balance_amount: "25",
      });
    });
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("textbox", {
        name: "Registration invitation code",
      }),
    ).toHaveValue("COMMUNITY-ACCESS-2026");
  });

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
