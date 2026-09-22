import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession } from "@/test/msw";

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

describe("ModelSetupPage", () => {
  it("presents the simplified pricing, channels, and routing sequence", async () => {
    seedAuthenticatedSession();
    renderAppAt("/admin/model-setup");

    expect(
      await screen.findByText("1. Pricing models"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("2. Channels"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("3. Model rules"),
    ).toBeInTheDocument();
    expect(screen.getByText("1 Operation rules")).toBeInTheDocument();
  });

  it("links directly to the focused hierarchical routing list", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/admin/model-setup");

    await user.click(
      await screen.findByRole("button", { name: "Manage routing" }),
    );
    expect(
      await screen.findByRole("heading", { name: "Operation rules" }),
    ).toBeInTheDocument();
    expect(window.location.pathname).toBe("/admin/routing/operation-rules");
  });
});
