import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL } from "@/test/fixtures";
import type { ApiKeyPolicyInput } from "@/api/types";

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

describe("ApiKeyPolicyDetailPage", () => {
  it("stores only the groups and channels users may choose", async () => {
    seedAuthenticatedSession();
    let submitted: ApiKeyPolicyInput | undefined;
    server.use(
      http.post("/console/v1/api-key-policies", async ({ request }) => {
        submitted = (await request.json()) as ApiKeyPolicyInput;
        return HttpResponse.json(
          {
            id: "00000000-0000-0000-0000-000000000099",
            correlation_id: "11111111-0000-0000-0000-000000000000",
          },
          { status: 201 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/api-key-policies/new");

    await user.type(await screen.findByLabelText(/^name$/i), "channel-only");
    await user.click(
      screen.getByRole("checkbox", { name: new RegExp(`^${CHANNEL.name}`, "i") }),
    );
    await user.click(screen.getByRole("button", { name: /create policy/i }));

    await waitFor(() => {
      expect(submitted).toEqual({
        name: "channel-only",
        allowed_group_ids: [],
        allowed_channel_ids: [CHANNEL.id],
        enabled: true,
      });
    });
  });
});
