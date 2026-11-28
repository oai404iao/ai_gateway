import { act, render, screen } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { AppProviders } from "./providers";
import { AppRouter } from "./router";
import { ADMIN_ACCESS_TOKEN, OWN_API_KEY } from "@/test/fixtures";
import { seedAuthenticatedSession, seedUserSession, server } from "@/test/msw";

it("discards fresh personal data when the authenticated owner changes on the same route", async () => {
  seedAuthenticatedSession();
  server.use(
    http.get("/console/v1/me/api-keys", ({ request }) =>
      HttpResponse.json({
        items: [
          {
            ...OWN_API_KEY,
            name:
              request.headers.get("Authorization") === `Bearer ${ADMIN_ACCESS_TOKEN}`
                ? "First owner credential"
                : "Second owner credential",
          },
        ],
        next_cursor: null,
        total: 1,
      }),
    ),
  );
  window.history.replaceState(null, "", "/api-keys");
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
  expect(await screen.findByRole("link", { name: "First owner credential" })).toBeVisible();
  act(seedUserSession);
  expect(screen.queryByText("First owner credential")).not.toBeInTheDocument();
  expect(await screen.findByRole("link", { name: "Second owner credential" })).toBeVisible();
  expect(screen.queryByRole("link", { name: "Management" })).not.toBeInTheDocument();
});
