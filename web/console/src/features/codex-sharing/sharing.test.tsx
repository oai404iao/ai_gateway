import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  CHANNEL,
  CODEX_QUOTA_GROUP,
  CONTROL_PLANE_USER,
  OWN_SHARING,
  SHARING_GROUP,
  SHARING_USAGE,
} from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";

function renderAt(path: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

function handlers() {
  server.use(
    http.get("/console/v1/codex-sharing-groups", () =>
      HttpResponse.json({ runtime_available: true, groups: [SHARING_GROUP] })),
    http.get("/console/v1/codex-sharing-groups/:id", () =>
      HttpResponse.json(SHARING_GROUP, { headers: { ETag: `"${SHARING_GROUP.updated_at}"` } })),
    http.get("/console/v1/codex-sharing-groups/:id/usage", () =>
      HttpResponse.json({ seats: [{ seat_number: 1, user_id: SHARING_GROUP.seats[0], usage: SHARING_USAGE }] })),
    http.get("/console/v1/routing/channel-groups", () => HttpResponse.json([CODEX_QUOTA_GROUP])),
    http.get("/console/v1/routing/channels", () => HttpResponse.json([{
      ...CHANNEL, api_format: "open_ai_responses", channel_group_id: CODEX_QUOTA_GROUP.id,
    }])),
  );
}

describe("Codex sharing", () => {
  it("shows separate USD windows and refreshes without resetting usage", async () => {
    let reads = 0;
    server.use(http.get("/console/v1/me/codex-sharing", () => {
      reads += 1;
      return HttpResponse.json(OWN_SHARING);
    }));
    renderAt("/codex-sharing");
    expect(await screen.findByText("$6.90")).toBeInTheDocument();
    expect(screen.getByText("$34.90")).toBeInTheDocument();
    expect(screen.queryByText(SHARING_GROUP.credential_id)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Refresh usage" }));
    await waitFor(() => expect(reads).toBeGreaterThan(1));
    expect(screen.getByText("$6.90")).toBeInTheDocument();
  });

  it("explains missing membership", async () => {
    server.use(http.get("/console/v1/me/codex-sharing", () => HttpResponse.json(null)));
    renderAt("/codex-sharing");
    expect(await screen.findByText("No sharing membership")).toBeInTheDocument();
  });

  it("keeps uncertain reservations visible until all affected windows reset", async () => {
    server.use(http.get("/console/v1/me/codex-sharing", () => HttpResponse.json({
      ...OWN_SHARING, usage: { ...SHARING_USAGE, uncertain: true, pending_requests: 1 },
    })));
    renderAt("/codex-sharing");
    expect(await screen.findByText(/all affected windows reset/)).toBeInTheDocument();
    expect(screen.getByText("$6.90")).toBeInTheDocument();
  });

  it("creates a paused binding with explicit scope warning and seeded empty seats", async () => {
    handlers();
    let submitted: unknown;
    let etag: string | null = "not-called";
    server.use(http.post("/console/v1/codex-sharing-groups", async ({ request }) => {
      submitted = await request.json();
      etag = request.headers.get("If-Match");
      return HttpResponse.json({ id: SHARING_GROUP.id, correlation_id: SHARING_GROUP.id }, { status: 201 });
    }));
    renderAt("/admin/codex-sharing/new");
    await screen.findByText(/Bindings cannot be undone/);
    await userEvent.type(screen.getByLabelText("Name"), "New car");
    await userEvent.click(screen.getByLabelText("Codex credential"));
    await userEvent.click(screen.getByRole("option", { name: CHANNEL.name }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submitted).toMatchObject({
      name: "New car", credential_id: CHANNEL.id,
      enabled: false, seats: [null], request_reservation_amount: "0.10",
    }));
    expect(etag).toBeNull();
  });

  it("saves all seeded Select fields without reselecting, using If-Match", async () => {
    handlers();
    let submitted: unknown;
    let etag: string | null = null;
    server.use(http.put("/console/v1/codex-sharing-groups/:id", async ({ request }) => {
      submitted = await request.json();
      etag = request.headers.get("If-Match");
      return HttpResponse.json({ id: SHARING_GROUP.id, correlation_id: SHARING_GROUP.id });
    }));
    renderAt(`/admin/codex-sharing/${SHARING_GROUP.id}`);
    await screen.findByDisplayValue(SHARING_GROUP.name);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submitted).toMatchObject({
      credential_id: SHARING_GROUP.credential_id,
      primary_limit_amount: "20", secondary_limit_amount: "100",
      seats: SHARING_GROUP.seats, enabled: true,
    }));
    expect(submitted).not.toHaveProperty("id");
    expect(etag).toBe(`"${SHARING_GROUP.updated_at}"`);
  });

  it("assigns a later vacant seat without filtering by user group", async () => {
    handlers();
    const otherGroupUser = {
      ...CONTROL_PLANE_USER,
      id: "00000000-0000-0000-0000-000000000899",
      email: "other-group@example.test",
      display_name: "Other group member",
      user_group_id: "00000000-0000-0000-0000-000000000898",
    };
    let submitted: unknown;
    server.use(
      http.get("/console/v1/users", () =>
        HttpResponse.json([CONTROL_PLANE_USER, otherGroupUser])),
      http.put("/console/v1/codex-sharing-groups/:id", async ({ request }) => {
        submitted = await request.json();
        return HttpResponse.json({
          id: SHARING_GROUP.id,
          correlation_id: SHARING_GROUP.id,
        });
      }),
    );
    renderAt(`/admin/codex-sharing/${SHARING_GROUP.id}`);
    await screen.findByDisplayValue(SHARING_GROUP.name);
    await userEvent.click(screen.getByLabelText("Seat 2"));
    await userEvent.click(screen.getByRole("option", { name: otherGroupUser.display_name }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(submitted).toMatchObject({
        seats: [CONTROL_PLANE_USER.id, otherGroupUser.id],
      }),
    );
  });

  it("reloads fresh configuration after a concurrent edit", async () => {
    handlers();
    let conflicted = false;
    server.use(
      http.get("/console/v1/codex-sharing-groups/:id", () => HttpResponse.json({
        ...SHARING_GROUP, name: conflicted ? "Changed elsewhere" : SHARING_GROUP.name,
      }, { headers: { ETag: `"${SHARING_GROUP.updated_at}"` } })),
      http.put("/console/v1/codex-sharing-groups/:id", () => {
        conflicted = true;
        return HttpResponse.json({ error: "conflict" }, { status: 409 });
      }),
    );
    renderAt(`/admin/codex-sharing/${SHARING_GROUP.id}`);
    await screen.findByDisplayValue(SHARING_GROUP.name);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByDisplayValue("Changed elsewhere")).toBeInTheDocument();
  });
});
