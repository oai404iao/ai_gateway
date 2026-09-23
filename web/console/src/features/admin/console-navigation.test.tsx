import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    ["/admin/model-setup", "/admin/models", "Model configuration"],
    ["/admin/catalog", "/admin/models?view=prices", "Model configuration"],
    ["/admin/routing/groups", "/admin/routing/channels?view=groups", "Channel configuration"],
    ["/admin/routing/logical-channels?group=id", "/admin/routing/channels?group=id", "Channel configuration"],
  ])("redirects %s to one canonical workspace", async (from, to, title) => {
    renderAt(from);
    expect(await screen.findByRole("heading", { level: 1, name: title })).toBeInTheDocument();
    await waitFor(() => expect(window.location.pathname + window.location.search).toBe(to));
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

  it("protects dirty channel tabs and explicit parent navigation in the production router", async () => {
    renderAt(`/admin/routing/logical-channels/${LOGICAL_CHANNEL.id}`);
    const user = userEvent.setup();
    const name = await screen.findByLabelText("Name");
    fireEvent.change(name, { target: { value: "Unsaved channel" } });
    await user.click(screen.getByRole("tab", { name: "Channel capabilities" }));
    expect(await screen.findByRole("alertdialog", { name: "Discard unsaved changes?" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(name).toHaveValue("Unsaved channel");
    await user.click(screen.getByRole("link", { name: "Back to channels" }));
    await user.click(await screen.findByRole("button", { name: "Discard changes" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Channel configuration" })).toBeInTheDocument();
  });

  it("uses a safe parent after direct entry with a malicious return target", async () => {
    renderAt("/admin/routing/capabilities/new?returnTo=https%3A%2F%2Fevil.test");
    const back = await screen.findByRole("link", { name: "Back to capabilities" });
    expect(back).toHaveAttribute("href", "/admin/routing/capabilities");
  });
});
