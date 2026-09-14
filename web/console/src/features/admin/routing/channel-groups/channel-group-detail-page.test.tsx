import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import {
  CHANNEL_DELETION_IMPACT,
  CHANNEL_GROUP,
} from "@/test/fixtures";
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
  it("edits the Codex sharing-only mode without enabling Images", async () => {
    seedAuthenticatedSession();
    let submitted: ChannelGroupInput | undefined;
    let ifMatch: string | null = null;
    const group = { ...CHANNEL_GROUP, connector_kind: "codex_oauth", api_format: "open_ai_responses", sharing_only: false };
    server.use(
      http.get("/console/v1/routing/channel-groups/:id", () => HttpResponse.json(group, {
        headers: { ETag: `"${group.updated_at}"` },
      })),
      http.put("/console/v1/routing/channel-groups/:id", async ({ request }) => {
        submitted = await request.json() as ChannelGroupInput;
        ifMatch = request.headers.get("If-Match");
        return HttpResponse.json({ id: group.id, correlation_id: group.id });
      }),
    );
    renderAppAt(`/admin/routing/channel-groups/${group.id}`);
    const toggle = await screen.findByRole("switch", { name: "Sharing only" });
    expect(toggle).not.toBeChecked();
    expect(screen.getByText(/both Responses and Images without enabling Images/)).toBeVisible();
    await userEvent.click(toggle);
    await userEvent.click(screen.getByRole("button", { name: /save group/i }));
    await waitFor(() => expect(submitted?.sharing_only).toBe(true));
    expect(ifMatch).toBe(`"${group.updated_at}"`);
  });

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
    expect(screen.queryByText("Priority")).not.toBeInTheDocument();
    expect(screen.queryByText("Selection strategy")).not.toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "Sharing only" })).not.toBeInTheDocument();
    const monitoring = screen.getByRole("switch", { name: "Status monitoring" });
    expect(monitoring).toBeChecked();
    await user.click(monitoring);
    await user.click(screen.getByRole("button", { name: /save group/i }));

    await waitFor(() => {
      expect(submitted?.status_statistics_enabled).toBe(false);
    });
    expect(submitted).not.toHaveProperty("priority");
    expect(submitted).not.toHaveProperty("selection_strategy");
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
    await user.click(
      await screen.findByRole("option", { name: "Zstandard (zstd)" }),
    );
    await user.click(screen.getByRole("button", { name: /save group/i }));

    await waitFor(() => {
      expect(submitted?.request_compression).toBe("zstd");
    });
  });

  it("keeps provider-managed groups out of the ordinary deletion path", async () => {
    seedAuthenticatedSession();
    const managedGroup = {
      ...CHANNEL_GROUP,
      id: "00000000-0000-0000-0000-000000000123",
      api_format: "open_ai_responses" as const,
      connector_kind: "codex_oauth",
      provider_managed: true,
    };
    server.use(
      http.get("/console/v1/routing/channel-groups/:id", () =>
        HttpResponse.json(managedGroup, {
          headers: { ETag: `"${managedGroup.updated_at}"` },
        }),
      ),
    );
    renderAppAt(`/admin/routing/channel-groups/${managedGroup.id}`);

    expect(await screen.findByText("Provider-managed group")).toBeVisible();
    expect(
      screen.getByText(
        "Provider-managed groups must use their connector lifecycle.",
      ),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Delete channel group" }),
    ).not.toBeInTheDocument();
  });

  it("previews authoritative dependencies before deleting an ordinary group", async () => {
    seedAuthenticatedSession();
    const impact = {
      ...CHANNEL_DELETION_IMPACT,
      resource_type: "channel_group" as const,
      resource_id: CHANNEL_GROUP.id,
    };
    let ifMatch: string | null = null;
    let confirmationToken: string | undefined;
    server.use(
      http.get(
        "/console/v1/routing/channel-groups/:id/deletion-impact",
        () => HttpResponse.json(impact),
      ),
      http.delete(
        "/console/v1/routing/channel-groups/:id",
        async ({ request }) => {
          ifMatch = request.headers.get("If-Match");
          confirmationToken = (
            (await request.json()) as { confirmation_token: string }
          ).confirmation_token;
          return HttpResponse.json({
            id: CHANNEL_GROUP.id,
            correlation_id: "99999999-0000-0000-0000-000000000012",
          });
        },
      ),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channel-groups/${CHANNEL_GROUP.id}`);

    await screen.findByRole("button", { name: "Delete channel group" });
    await user.click(
      screen.getByRole("button", { name: "Delete channel group" }),
    );
    const dialog = await screen.findByRole("alertdialog", {
      name: "Delete channel group?",
    });
    expect(
      within(dialog).getByText(`Channels to delete (${impact.channels.length})`),
    ).toBeVisible();
    expect(within(dialog).getByText(impact.channels[0].name)).toBeVisible();
    expect(within(dialog).getByText(/will be disabled/i)).toBeVisible();
    expect(within(dialog).getByText("Tiers removed: 0")).toBeVisible();

    await user.click(
      within(dialog).getByRole("button", { name: "Delete channel group" }),
    );
    await waitFor(() => {
      expect(confirmationToken).toBe(impact.confirmation_token);
    });
    expect(ifMatch).toBe(`"${CHANNEL_GROUP.updated_at}"`);
    expect(window.location.pathname).toBe("/admin/routing/channels");
  });
});
