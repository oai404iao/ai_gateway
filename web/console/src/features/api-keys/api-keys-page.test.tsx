import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { OWN_API_KEY } from "@/test/fixtures";

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

describe("ApiKeysPage", () => {
  it("creates a key and shows the one-time secret dialog", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");

    expect(await screen.findByText(OWN_API_KEY.name)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /new api key/i }));
    const nameField = await screen.findByLabelText(/name/i);
    await user.type(nameField, "spec key");
    await user.click(screen.getByRole("button", { name: /create key/i }));

    // The one-time secret dialog appears with the secret and a copy button.
    expect(await screen.findByText(/save it now/i)).toBeInTheDocument();
    expect(await screen.findByText(/sk-ag-test-secret-only-once/i)).toBeInTheDocument();
  });

  it("navigates to a key detail page on row click", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt("/api-keys");
    const row = await screen.findByText(OWN_API_KEY.name);
    await user.click(row);
    // The detail page renders the danger-zone revoke button.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /revoke api key/i })).toBeInTheDocument();
    });
  });

  it("surfaces an optimistic-concurrency conflict as a toast", async () => {
    seedAuthenticatedSession();
    // Own API key detail GET returns a stable ETag; PUT with a mismatched
    // If-Match yields 409.
    let putHit = false;
    server.use(
      http.get("/console/v1/me/api-keys/:id", () =>
        HttpResponse.json(OWN_API_KEY, {
          headers: { ETag: `"v1"` },
        }),
      ),
      http.put("/console/v1/me/api-keys/:id", () => {
        putHit = true;
        return HttpResponse.json(
          { error: "Console operation rejected" },
          { status: 409 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/api-keys/${OWN_API_KEY.id}`);

    // The detail form loads with reset (valid) values; renaming and saving
    // sends If-Match from the GET ETag, and the overridden PUT returns 409.
    const nameField = await screen.findByLabelText(/name/i);
    await user.clear(nameField);
    await user.type(nameField, "renamed");
    await user.click(screen.getByRole("button", { name: /save changes/i }));

    await waitFor(() => {
      expect(putHit).toBe(true);
      expect(screen.getByText(/changed by another session/i)).toBeInTheDocument();
    }, { timeout: 3000 });
  });
});
