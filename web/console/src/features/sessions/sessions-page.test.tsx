import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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
  it("identifies devices and revokes a selected browser session", async () => {
    seedAuthenticatedSession();
    let revokedId: string | null = null;
    server.use(
      http.delete("/console/v1/me/sessions/:id", ({ params }) => {
        revokedId = String(params.id);
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    expect(await screen.findByText("Safari 18 · macOS")).toBeInTheDocument();
    expect(screen.getByText("Firefox 128 · Windows")).toBeInTheDocument();
    expect(screen.getByText("Current device")).toBeInTheDocument();
    expect(screen.queryByText("Revoked session")).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Sign out Firefox 128 · Windows" }),
    );
    const dialog = screen.getByRole("alertdialog");
    expect(
      within(dialog).getByText(/immediately ends that browser session/i),
    ).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /^sign out$/i }));

    await waitFor(() => {
      expect(revokedId).toBe("00000000-0000-0000-0000-0000000000a3");
      expect(screen.getByText("Device signed out")).toBeInTheDocument();
    });
  });

  it("revokes every other active session while preserving the current one", async () => {
    seedAuthenticatedSession();
    let collectionDelete = false;
    server.use(
      http.delete("/console/v1/me/sessions", () => {
        collectionDelete = true;
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    await user.click(await screen.findByRole("button", { name: "Sign out other devices" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/keeps your current device signed in/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /^sign out$/i }));

    await waitFor(() => {
      expect(collectionDelete).toBe(true);
      expect(screen.getByText("Other devices signed out")).toBeInTheDocument();
    });
  });

  it("returns to sign in after revoking the current session", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/account/sessions");

    await user.click(await screen.findByRole("button", { name: "Sign out this device" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/current Console session/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /^sign out$/i }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^sign in$/i })).toBeInTheDocument();
    });
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
