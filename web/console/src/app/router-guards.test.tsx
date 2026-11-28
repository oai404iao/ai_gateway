import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { LoginPage } from "@/features/auth/login-page";
import { ADMIN_ACCESS_TOKEN, ADMIN_USER } from "@/test/fixtures";
import { seedUserSession, server } from "@/test/msw";

describe("Router guards", () => {
  it("preserves the requested deep link across login", async () => {
    server.use(
      http.post("/console/v1/auth/refresh", () =>
        HttpResponse.json({ error: "unauthorized", message: "Unauthorized" }, { status: 401 }),
      ),
      http.post("/console/v1/auth/login", () =>
        HttpResponse.json({
          access_token: ADMIN_ACCESS_TOKEN,
          token_type: "Bearer",
          expires_in: 900,
          user: ADMIN_USER,
        }),
      ),
    );

    window.history.replaceState({}, "", "/usage/request-logs?limit=25#details");
    render(
      <AppProviders>
        <MemoryRouter initialEntries={["/usage/request-logs?limit=25#details"]}>
          <AppRouter />
        </MemoryRouter>
      </AppProviders>,
    );

    const emailInput = await screen.findByLabelText("Email");
    const user = userEvent.setup();
    await user.type(emailInput, "admin@example.com");
    await user.type(screen.getByLabelText("Password"), "correct-horse-battery");
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByRole("heading", { name: "Request Logs" })).toBeInTheDocument();
  });

  it("ignores external redirect targets in login state", async () => {
    seedUserSession();
    render(
      <AppProviders>
        <MemoryRouter
          initialEntries={[{ pathname: "/login", state: { from: "https://evil.example/phish" } }]}
        >
          <Routes>
            <Route path="/login" element={<LoginPage />} />
            <Route path="/" element={<div>Overview landing</div>} />
          </Routes>
        </MemoryRouter>
      </AppProviders>,
    );

    expect(await screen.findByText("Overview landing")).toBeInTheDocument();
  });

  it("renders a 403 access denied state when a non-admin visits an admin route", async () => {
    seedUserSession();
    render(
      <AppProviders>
        <MemoryRouter initialEntries={["/admin/users"]}>
          <AppRouter />
        </MemoryRouter>
      </AppProviders>,
    );

    expect(await screen.findByText("Access denied")).toBeInTheDocument();
    expect(screen.getByText("Administrator access is required to view this page.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute("href", "/");
  });
});
