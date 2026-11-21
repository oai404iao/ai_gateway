import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession, server } from "@/test/msw";
import { OWN_CODEX_QUOTA, OWN_CODEX_QUOTA_HISTORY } from "@/test/fixtures";

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
  it("distinguishes anonymous accounts with the same plan and opens the corresponding history", async () => {
    seedAuthenticatedSession();
    const id = "00000000-0000-0000-0000-00000000d010";
    const historyIds: string[] = [];
    server.use(
      http.get("/console/v1/me/codex-quotas", () => HttpResponse.json([
        OWN_CODEX_QUOTA, {...OWN_CODEX_QUOTA, id, name: id},
      ])),
      http.get("/console/v1/me/codex-quotas/:id/windows", ({params}) => {
        historyIds.push(String(params.id));
        return HttpResponse.json({...OWN_CODEX_QUOTA_HISTORY, credential_id: id, name: id});
      }),
    );
    renderAppAt("/codex-quotas");
    const name = `Codex account · plus · ${id.slice(-6)}`;
    const historyButton = await screen.findByRole("button", {name: `View quota history for ${name}`});
    expect(screen.queryByText(id)).not.toBeInTheDocument();
    expect(historyIds).toEqual([]);
    await userEvent.click(historyButton);
    expect(await screen.findByRole("dialog", {name: `Quota window history for ${name}`})).toBeVisible();
    await screen.findByText("Natural reset");
    expect(historyIds).toEqual([id]);
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
