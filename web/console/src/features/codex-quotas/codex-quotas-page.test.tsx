import { render, screen } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession, server } from "@/test/msw";

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

describe("CodexQuotasPage", () => {
  it("explains when the user group has no quota visibility", async () => {
    seedAuthenticatedSession();
    server.use(
      http.get("/console/v1/me/codex-quotas", () => HttpResponse.json([])),
    );
    renderAppAt("/codex-quotas");

    expect(
      await screen.findByText("No Codex quota access"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Your user group has not been granted access to any Codex quota groups.",
      ),
    ).toBeInTheDocument();
  });
});
