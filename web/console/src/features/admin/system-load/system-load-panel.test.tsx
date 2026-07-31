import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession } from "@/test/msw";

function renderApp() {
  window.history.replaceState({}, "", "/admin/system-load");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("SystemLoadPanel", () => {
  it("shows Images edit spool capacity and active files", async () => {
    seedAuthenticatedSession();
    renderApp();

    const title = await screen.findByText("Images edit body spool");
    const card = title.closest('[data-slot="card"]');
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByText("2")).toBeInTheDocument();
    expect(
      within(card as HTMLElement).getByText("500 GiB"),
    ).toBeInTheDocument();
    expect(
      within(card as HTMLElement).getByText("No failures"),
    ).toBeInTheDocument();
  });
});
