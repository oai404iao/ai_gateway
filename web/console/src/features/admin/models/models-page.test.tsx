import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { MODEL } from "@/test/fixtures";
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

describe("ModelsPage", () => {
  it("provides a direct pricing action without opening the general model editor", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/admin/models?mode=table");

    await user.click(
      await screen.findByRole("button", { name: /configure pricing/i }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe(`/admin/models/${MODEL.id}/pricing`);
    });
    expect(
      await screen.findByText(/multiplier calculator/i),
    ).toBeInTheDocument();
  });
});
