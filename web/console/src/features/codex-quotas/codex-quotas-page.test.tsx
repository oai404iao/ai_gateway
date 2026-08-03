import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  CODEX_QUOTA_GROUP,
  OWN_CODEX_QUOTA,
} from "@/test/fixtures";
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
  it("shows only sanitized read-only quota data and history", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/codex-quotas");

    expect(
      await screen.findByRole("heading", { name: "Codex quotas" }),
    ).toBeInTheDocument();
    expect(await screen.findByText(CODEX_QUOTA_GROUP.id)).toBeInTheDocument();
    expect(screen.getByText(OWN_CODEX_QUOTA.name)).toBeInTheDocument();
    expect(screen.getByText("plus")).toBeInTheDocument();
    expect(screen.getByText("42%")).toBeInTheDocument();
    expect(screen.queryByText("Personal Plus")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /refresh quota/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /reset quota/i }),
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: `View quota history for ${OWN_CODEX_QUOTA.name}`,
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("heading", {
        name: `Quota window history for ${OWN_CODEX_QUOTA.name}`,
      }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("Natural reset")).toBeInTheDocument();
    expect(within(dialog).getByText("5% → 42%")).toBeInTheDocument();
    expect(
      within(dialog).queryByRole("button", { name: /view costs/i }),
    ).not.toBeInTheDocument();
  });

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
