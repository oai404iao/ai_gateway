import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouterProvider } from "@/app/router";
import { LOGICAL_CHANNEL, ROUTING_GROUP } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

function renderAt(path: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(<AppProviders><AppRouterProvider /></AppProviders>);
}

describe("Console navigation contracts", () => {
  it.each([
    ["/admin/models?view=prices", "Price sync", "Model configuration"],
    ["/admin/routing/channels?view=groups", "Group configuration", "Channel configuration"],
  ])("opens the selected workspace tab at %s", async (path, tab, title) => {
    renderAt(path);
    expect(await screen.findByRole("heading", { level: 1, name: title })).toBeInTheDocument();
    expect(await screen.findByRole("tab", { name: tab, selected: true })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole("navigation", { name: "Model routing configuration" })).not.toBeInTheDocument();
  });

  it("returns to the filtered list and page through an accessible identity link", async () => {
    const rows = Array.from({ length: 20 }, (_, i) => ({
      ...LOGICAL_CHANNEL, id: `channel-${i}`, name: `Channel ${i}`,
    }));
    server.use(http.get("/console/v1/routing/logical-channels", () =>
      HttpResponse.json([...rows, LOGICAL_CHANNEL])));
    const origin = `/admin/routing/channels?group=${ROUTING_GROUP.id}&page=2`;
    renderAt(origin);
    const user = userEvent.setup();
    const link = await screen.findByRole("link", { name: LOGICAL_CHANNEL.name });
    link.focus();
    await user.keyboard("{Enter}");
    const back = await screen.findByRole("link", { name: "Back to channels" });
    expect(back).toHaveAttribute("href", origin);
    expect(screen.getAllByRole("link", { name: /^Back to/ })).toHaveLength(1);
    await user.click(back);
    expect(await screen.findByRole("link", { name: LOGICAL_CHANNEL.name })).toBeInTheDocument();
    expect(window.location.search).toBe(`?group=${ROUTING_GROUP.id}&page=2`);
    expect(screen.queryByRole("link", { name: "Channel 0" })).not.toBeInTheDocument();
  });

  it("uses a safe parent after direct entry with a malicious return target", async () => {
    renderAt("/admin/routing/capabilities/new?returnTo=https%3A%2F%2Fevil.test");
    const back = await screen.findByRole("link", { name: "Back to capabilities" });
    expect(back).toHaveAttribute("href", "/admin/routing/capabilities");
  });

  it("preserves expanded management navigation group across route changes", async () => {
    renderAt("/admin/models");
    const user = userEvent.setup();
    const usersToggle = await screen.findByRole("button", { name: "Members and access" });
    expect(usersToggle).toHaveAttribute("aria-expanded", "false");

    await user.click(usersToggle);
    expect(usersToggle).toHaveAttribute("aria-expanded", "true");

    await user.click(screen.getByRole("link", { name: "Users" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Users" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Members and access" })).toHaveAttribute("aria-expanded", "true");
  });
});
