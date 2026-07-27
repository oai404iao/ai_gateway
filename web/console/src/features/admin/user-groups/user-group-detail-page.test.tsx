import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { USER_GROUP } from "@/test/fixtures";
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

describe("UserGroupDetailPage", () => {
  it("protects built-in default groups from deletion", async () => {
    seedAuthenticatedSession();
    renderAppAt(`/admin/user-groups/${USER_GROUP.id}`);

    expect(await screen.findByText("Protected default group")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Delete user group" }),
    ).not.toBeInTheDocument();
  });

  it("confirms deletion of an empty custom group", async () => {
    seedAuthenticatedSession();
    const customGroup = {
      ...USER_GROUP,
      id: "00000000-0000-0000-0000-000000000199",
      name: "Contractors",
      system_role: null,
      member_count: 0,
    };
    let deleted = false;
    server.use(
      http.get("/console/v1/user-groups/:id", () =>
        HttpResponse.json(customGroup, {
          headers: { ETag: `"${customGroup.updated_at}"` },
        }),
      ),
      http.delete("/console/v1/user-groups/:id", ({ request }) => {
        deleted = request.headers.get("If-Match") === `"${customGroup.updated_at}"`;
        return HttpResponse.json({
          id: customGroup.id,
          correlation_id: "00000000-0000-0000-0000-000000000054",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/user-groups/${customGroup.id}`);

    await user.click(
      await screen.findByRole("button", { name: "Delete user group" }),
    );
    const confirmation = await screen.findByRole("alertdialog");
    await user.click(
      within(confirmation).getByRole("button", { name: "Delete user group" }),
    );

    await waitFor(() => expect(deleted).toBe(true));
  });
});
