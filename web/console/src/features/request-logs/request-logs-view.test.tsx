import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";

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

describe("RequestLogsView", () => {
  it("sends the filter bar values as server-side request-log query parameters", async () => {
    seedAuthenticatedSession();
    const queries: URLSearchParams[] = [];
    server.use(
      http.get("/console/v1/me/request-logs", ({ request }) => {
        queries.push(new URL(request.url).searchParams);
        return HttpResponse.json([]);
      }),
    );

    const user = userEvent.setup();
    renderAppAt("/usage/request-logs");

    const model = await screen.findByLabelText("Model");
    await user.type(model, "gpt-4o-mini");
    await user.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(queries.some((query) => query.get("model") === "gpt-4o-mini")).toBe(true);
    });
  });
});
