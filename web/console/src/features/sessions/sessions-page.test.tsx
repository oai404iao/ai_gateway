import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { ACTIVE_SESSION, OTHER_ACTIVE_SESSION } from "@/test/fixtures";

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  return render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("LoginPage", () => {
  it("shows a server error toast on 401", async () => {
    server.use(
      http.post("/console/v1/auth/refresh", () =>
        HttpResponse.json({ error: "unauthorized" }, { status: 401 }),
      ),
      http.post("/console/v1/auth/login", () =>
        HttpResponse.json({ error: "unauthorized" }, { status: 401 }),
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/login");
    const email = await screen.findByLabelText(/email/i);
    await user.type(email, "admin@example.com");
    await user.type(screen.getByLabelText(/password/i), "a-very-long-password");
    await user.click(screen.getByRole("button", { name: /sign in/i }));

    await waitFor(() => {
      expect(screen.getByText(/unauthorized/i)).toBeInTheDocument();
    });
  });
});

describe("SessionsPage", () => {
  it("signs out other devices and displays the refreshed active and revoked sessions", async () => {
    seedAuthenticatedSession();
    const deletes: string[] = [];
    let other = OTHER_ACTIVE_SESSION;
    server.use(
      http.get("/console/v1/me/sessions", () => HttpResponse.json([ACTIVE_SESSION, other])),
      http.delete("/console/v1/me/sessions", () => {
        deletes.push("collection");
        other = { ...other, state: "revoked", revoked_at: "2026-08-01T00:00:00Z" };
        return new HttpResponse(null, { status: 204 });
      }),
      http.delete("/console/v1/me/sessions/:id", ({ params }) => {
        deletes.push(String(params.id));
        return HttpResponse.json({ error: "unexpected_session_delete" }, { status: 400 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    await screen.findByRole("button", { name: /sign out firefox/i });
    await user.click(screen.getByRole("button", { name: "Sign out other devices" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/keeps your current device signed in/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /^sign out$/i }));

    expect(await screen.findByText("Other devices signed out")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("button", { name: /sign out firefox/i })).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Sign out this device" })).toBeEnabled();
    expect(screen.getByText("Session 000000a1")).toBeVisible();
    expect(screen.getByRole("button", { name: "Sign out other devices" })).toBeDisabled();
    expect(screen.queryByText("Session 000000a3")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show history" }));
    expect(await screen.findByText("Revoked session")).toBeVisible();
    expect(screen.getByText("Session 000000a3")).toBeVisible();
    expect(deletes).toEqual(["collection"]);
  });

  it("returns to sign in after revoking the current session", async () => {
    seedAuthenticatedSession();
    const deletes: string[] = [];
    server.use(
      http.delete("/console/v1/me/sessions", () => {
        deletes.push("collection");
        return HttpResponse.json({ error: "unexpected_collection_delete" }, { status: 400 });
      }),
      http.delete("/console/v1/me/sessions/:id", ({ params }) => {
        deletes.push(String(params.id));
        return params.id === ACTIVE_SESSION.id
          ? new HttpResponse(null, { status: 204 })
          : HttpResponse.json({ error: "unexpected_session_delete" }, { status: 404 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    await user.click(await screen.findByRole("button", { name: "Sign out this device" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/current Console session/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /^sign out$/i }));

    expect(await screen.findByRole("button", { name: /^sign in$/i })).toBeInTheDocument();
    expect(deletes).toEqual([ACTIVE_SESSION.id]);
  });

  it("shows expired and revoked sessions only after history is expanded", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    expect(await screen.findByRole("button", { name: "Show history" })).toBeVisible();
    expect(screen.queryByText("Revoked session")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show history" }));
    expect(await screen.findByText("Revoked session")).toBeVisible();
    expect(screen.getByText("Expired session")).toBeVisible();
    expect(screen.getByText("curl 8 · Linux")).toBeVisible();
    expect(screen.getByText("Unknown browser")).toBeVisible();
  });

  it("renders the empty state when there are no sessions", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/me/sessions", () => HttpResponse.json([])),
    );
    renderAppAt("/account/sessions");
    expect(
      await screen.findByText("No login sessions", {}, { timeout: 3000 }),
    ).toBeInTheDocument();
  });
});
