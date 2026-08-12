import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL_GROUP } from "@/test/fixtures";
import type { ChannelGroupInput } from "@/api/types";

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

describe("ChannelGroupDetailPage", () => {
  it("edits group-level status monitoring", async () => {
    seedAuthenticatedSession();
    let submitted: ChannelGroupInput | undefined;
    server.use(
      http.put("/console/v1/routing/channel-groups/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelGroupInput;
        return HttpResponse.json({
          id: CHANNEL_GROUP.id,
          correlation_id: "99999999-0000-0000-0000-000000000001",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channel-groups/${CHANNEL_GROUP.id}`);

    await waitFor(() => {
      expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(
        CHANNEL_GROUP.name,
      );
    });
    const monitoring = screen.getByRole("switch", { name: "Status monitoring" });
    expect(monitoring).toBeChecked();
    await user.click(monitoring);
    await user.click(screen.getByRole("button", { name: /save group/i }));

    await waitFor(() => {
      expect(submitted?.status_statistics_enabled).toBe(false);
    });
  });

  it("edits Responses request compression", async () => {
    seedAuthenticatedSession();
    const responsesGroup = {
      ...CHANNEL_GROUP,
      id: "00000000-0000-0000-0000-000000000122",
      name: "responses-compression",
      api_format: "open_ai_responses" as const,
      request_compression: "default" as const,
    };
    let submitted: ChannelGroupInput | undefined;
    server.use(
      http.get("/console/v1/routing/channel-groups/:id", () =>
        HttpResponse.json(responsesGroup, {
          headers: { ETag: `"${responsesGroup.updated_at}"` },
        }),
      ),
      http.put("/console/v1/routing/channel-groups/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelGroupInput;
        return HttpResponse.json({
          id: responsesGroup.id,
          correlation_id: "99999999-0000-0000-0000-000000000002",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channel-groups/${responsesGroup.id}`);

    await waitFor(() => {
      expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(
        responsesGroup.name,
      );
    });
    await user.click(screen.getByRole("combobox", { name: "Request compression" }));
    await user.click(screen.getByRole("option", { name: "Zstandard (zstd)" }));
    await user.click(screen.getByRole("button", { name: /save group/i }));

    await waitFor(() => {
      expect(submitted?.request_compression).toBe("zstd");
    });
  });
});
