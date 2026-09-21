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

describe("configuration navigation", () => {
  it("moves between the focused model configuration lists", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/admin/models");

    const navigation = await screen.findByRole("navigation", {
      name: "Model routing configuration",
    });
    await user.click(
      screen.getByRole("button", { name: "Operation rules" }),
    );

    expect(window.location.pathname).toBe("/admin/routing/operation-rules");
    expect(navigation).toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "Operation rules" }),
    ).toBeInTheDocument();
  });
});
