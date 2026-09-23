import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { CHANNEL_CAPABILITY, LOGICAL_CHANNEL, ROUTING_GROUP } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

function renderAppAt(path: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(<AppProviders><BrowserRouter><AppRouter /></BrowserRouter></AppProviders>);
}

describe("Channel configuration", () => {
  it("filters a flat channel list by group and edits capabilities inside the channel", async () => {
    const secondGroup = { ...ROUTING_GROUP, id: "00000000-0000-4000-8000-000000000097", name: "Second group" };
    const secondChannel = { ...LOGICAL_CHANNEL, id: "00000000-0000-4000-8000-000000000099", name: "Second channel", group_id: secondGroup.id };
    server.use(
      http.get("/console/v1/routing/groups", () => HttpResponse.json([ROUTING_GROUP, secondGroup])),
      http.get("/console/v1/routing/logical-channels", () =>
        HttpResponse.json([LOGICAL_CHANNEL, secondChannel])),
      http.get("/console/v1/routing/capabilities", () => HttpResponse.json([
        CHANNEL_CAPABILITY,
        { ...CHANNEL_CAPABILITY, id: "00000000-0000-4000-8000-000000000098",
          channel_id: secondChannel.id, settings: { ...CHANNEL_CAPABILITY.settings, operation: "images_edit" } },
      ])),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/routing/channels");
    expect(await screen.findByRole("heading", { name: "Channel configuration" })).toBeInTheDocument();
    const row = await screen.findByRole("row", { name: new RegExp(LOGICAL_CHANNEL.name) });
    expect(screen.getAllByRole("row")).toHaveLength(3);
    expect(screen.queryByRole("heading", { name: "Channel capabilities" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("combobox", { name: "Channel group" }));
    await user.click(await screen.findByRole("option", { name: ROUTING_GROUP.name }));
    expect(screen.queryByRole("row", { name: /Second channel/ })).not.toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("group")).toBe(ROUTING_GROUP.id);
    await user.click(within(row).getByText(LOGICAL_CHANNEL.name));
    expect(await screen.findByRole("button", { name: "Save channel" })).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Channel capabilities" }));
    expect(await screen.findByRole("heading", { name: "Channel capabilities" })).toBeInTheDocument();
    expect(screen.queryByText("Images edit")).not.toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "New capability" }));
    await waitFor(() => expect(window.location.pathname).toBe(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`));
    expect(new URLSearchParams(window.location.search).get("capability")).toBe("new");
    expect(await screen.findByRole("combobox", { name: "Logical channel" })).toHaveTextContent(LOGICAL_CHANNEL.name);
    expect(screen.getByRole("combobox", { name: "Logical channel" })).toBeDisabled();
  });

  it("redirects the old selected-channel URL into the channel's capabilities tab", async () => {
    renderAppAt(`/admin/routing/channels?channel=${LOGICAL_CHANNEL.id}`);
    expect(await screen.findByRole("heading", { name: "Channel capabilities" })).toBeInTheDocument();
    expect(window.location.pathname).toBe(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`);
  });

  it("exposes group management inside channel configuration", async () => {
    const user = userEvent.setup();
    renderAppAt("/admin/routing/channels");
    await user.click(await screen.findByRole("tab", { name: "Group configuration" }));
    expect(await screen.findByRole("link", { name: "New group" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("view")).toBe("groups");
    expect(screen.getByRole("link", { name: "Channel configuration" })).toHaveAttribute("data-active");
  });
});
