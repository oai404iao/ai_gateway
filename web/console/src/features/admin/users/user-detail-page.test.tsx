import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import type { UserUpdateInput } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CONTROL_PLANE_USER } from "@/test/fixtures";

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
  it("updates the user's personal WebSocket setting", async () => {
    seedAuthenticatedSession();
    const managedUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000094",
      role: "user" as const,
      websocket_enabled: false,
    };
    let submitted: UserUpdateInput | undefined;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(managedUser, {
          headers: { ETag: `"${managedUser.updated_at}"` },
        }),
      ),
      http.patch("/console/v1/users/:id", async ({ request }) => {
        submitted = (await request.json()) as UserUpdateInput;
        return HttpResponse.json({
          id: managedUser.id,
          correlation_id: "11111111-0000-0000-0000-000000000094",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/users/${managedUser.id}`);

    await user.click(
      await screen.findByRole("switch", {
        name: "Enable Responses WebSocket",
      }),
    );
    await user.click(
      screen.getByRole("button", {
        name: "Save personal settings",
      }),
    );

    await waitFor(() => {
      expect(submitted).toEqual({ websocket_enabled: true });
    });
  });

  it("updates only the balance without resubmitting account or status fields", async () => {
    seedAuthenticatedSession();
    const managedUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000099",
      email: "managed@example.test",
      display_name: "Managed Admin",
      status: "suspended",
    };
    let submitted: UserUpdateInput | undefined;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(managedUser, {
          headers: { ETag: `"${managedUser.updated_at}"` },
        }),
      ),
      http.patch("/console/v1/users/:id", async ({ request }) => {
        submitted = (await request.json()) as UserUpdateInput;
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
    await user.click(screen.getByRole("button", { name: /update balance/i }));

    await waitFor(() => {
      expect(submitted).toEqual({ balance_amount: "25.5" });
    });
  });

  it("keeps invited users pending when account details are edited", async () => {
    seedAuthenticatedSession();
    const invitedUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000098",
      email: "invited@example.test",
      display_name: "Invited User",
      role: "user" as const,
      status: "invited",
      can_reissue_invitation: true,
      balance_amount: "30.00",
    };
    let submitted: UserUpdateInput | undefined;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(invitedUser, {
          headers: { ETag: `"${invitedUser.updated_at}"` },
        }),
      ),
      http.patch("/console/v1/users/:id", async ({ request }) => {
        submitted = (await request.json()) as UserUpdateInput;
        return HttpResponse.json({
          id: invitedUser.id,
          correlation_id: "33333333-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/users/${invitedUser.id}`);

    expect(await screen.findByText("Pending invitation activation")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /update status/i })).not.toBeInTheDocument();
    const displayName = screen.getByDisplayValue(invitedUser.display_name);
    await user.clear(displayName);
    await user.type(displayName, "Renamed Invitee");
    await user.click(screen.getByRole("button", { name: /save account details/i }));

    await waitFor(() => {
      expect(submitted).toEqual({ display_name: "Renamed Invitee" });
    });
  });

  it("recovers a disabled never-activated user with a replacement invitation", async () => {
    seedAuthenticatedSession();
    const disabledInvitee = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000097",
      email: "disabled-invitee@example.test",
      display_name: "Disabled Invitee",
      role: "user" as const,
      status: "disabled",
      can_reissue_invitation: true,
    };
    let reissued = false;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(disabledInvitee, {
          headers: { ETag: `"${disabledInvitee.updated_at}"` },
        }),
      ),
      http.post("/console/v1/users/:id/invitation", () => {
        reissued = true;
        return HttpResponse.json(
          {
            id: "00000000-0000-0000-0000-000000000051",
            user_id: disabledInvitee.id,
            invitation_token: "replacement-invitation-token",
            expires_at: "2026-08-01T00:00:00.000Z",
            correlation_id: "00000000-0000-0000-0000-000000000052",
          },
          { status: 201 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/users/${disabledInvitee.id}`);

    expect(await screen.findByText("Invitation recovery available")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /reissue invitation/i }));
    const confirmation = await screen.findByRole("alertdialog");
    await user.click(
      within(confirmation).getByRole("button", { name: /reissue invitation/i }),
    );

    expect(
      await screen.findByDisplayValue("replacement-invitation-token"),
    ).toBeInTheDocument();
    expect(reissued).toBe(true);
  });

  it("requires confirmation before deleting and anonymizing a user", async () => {
    seedAuthenticatedSession();
    const managedUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000096",
      email: "delete-me@example.test",
      display_name: "Delete Me",
      role: "user" as const,
    };
    let deleted = false;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(managedUser, {
          headers: { ETag: `"${managedUser.updated_at}"` },
        }),
      ),
      http.delete("/console/v1/users/:id", ({ request }) => {
        deleted = request.headers.get("If-Match") === `"${managedUser.updated_at}"`;
        return HttpResponse.json({
          id: managedUser.id,
          correlation_id: "00000000-0000-0000-0000-000000000053",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/users/${managedUser.id}`);

    await user.click(await screen.findByRole("button", { name: "Delete user" }));
    const confirmation = await screen.findByRole("alertdialog");
    expect(
      within(confirmation).getByText(/revokes every session, invitation, and API key/i),
    ).toBeInTheDocument();
    await user.click(
      within(confirmation).getByRole("button", { name: "Delete user" }),
    );

    await waitFor(() => expect(deleted).toBe(true));
  });

  it("reauthenticates the administrator and shows a temporary password once", async () => {
    seedAuthenticatedSession();
    const managedUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000095",
      email: "password-reset@example.test",
      display_name: "Password Reset Target",
      role: "user" as const,
    };
    let submitted: unknown;
    server.use(
      http.get("/console/v1/users/:id", () =>
        HttpResponse.json(managedUser, {
          headers: { ETag: `"${managedUser.updated_at}"` },
        }),
      ),
      http.post("/console/v1/users/:id/temporary-password", async ({ request }) => {
        submitted = await request.json();
        return HttpResponse.json(
          {
            user_id: managedUser.id,
            temporary_password: "AGW-generated-temporary-password",
            expires_at: "2099-08-02T00:00:00.000Z",
            correlation_id: "00000000-0000-0000-0000-000000000054",
          },
          { status: 201 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/users/${managedUser.id}`);

    await user.click(
      await screen.findByRole("button", { name: "Generate temporary password" }),
    );
    const confirmation = await screen.findByRole("alertdialog");
    await user.type(
      within(confirmation).getByLabelText("Your current password"),
      "administrator-password",
    );
    await user.click(
      within(confirmation).getByRole("button", {
        name: "Generate temporary password",
      }),
    );

    await waitFor(() => {
      expect(submitted).toEqual({ current_password: "administrator-password" });
    });
    expect(
      await screen.findByDisplayValue("AGW-generated-temporary-password"),
    ).toBeInTheDocument();
  });
});
