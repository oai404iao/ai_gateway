import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";

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
  it("submits credentials and redirects to /account", async () => {
    // Keep the boot refresh failing so the login form stays visible until the
    // user submits; the /auth/login handler still succeeds and sets the
    // session, after which the page redirects to /account.
    server.use(
      http.post("/console/v1/auth/refresh", () =>
        HttpResponse.json({ error: "unauthorized" }, { status: 401 }),
      ),
    );
    const user = userEvent.setup();
    renderAppAt("/login");

    const email = await screen.findByLabelText(/email/i);
    await user.type(email, "admin@example.com");
    await user.type(screen.getByLabelText(/password/i), "a-very-long-password");
    await user.click(screen.getByRole("button", { name: /sign in/i }));

    // The router redirects authenticated users to /account, which renders the
    // Console layout and the profile page heading.
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /^profile$/i })).toBeInTheDocument();
    });
  });

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
  it("lists sessions and revokes the active one", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    // The page lists one active and one revoked session (rendered as badges).
    expect(await screen.findByText("Active")).toBeInTheDocument();
    expect(await screen.findByText("Revoked")).toBeInTheDocument();

    // Revoke triggers a DELETE and a success toast.
    const revokeButton = await screen.findByRole("button", {
      name: /revoke newest active session/i,
    });
    await user.click(revokeButton);
    await user.click(screen.getByRole("button", { name: /^revoke$/i }));

    await waitFor(() => {
      expect(screen.getByText(/session revoked/i)).toBeInTheDocument();
    });
  });

  it("renders the empty state when there are no sessions", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/me/sessions", () => HttpResponse.json([])),
    );
    renderAppAt("/account/sessions");
    expect(await screen.findByText("No sessions", {}, { timeout: 3000 })).toBeInTheDocument();
  });
});
