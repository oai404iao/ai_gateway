import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { CHANNEL, CHANNEL_GROUP, MODEL, MODEL_RULE } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";
import { configurationPath } from "./configuration-graph";

function renderAt(path: string) {
  seedAuthenticatedSession();
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

describe("configuration workbench", () => {
  it("traces a route to price, supply and back without opening editors", async () => {
    const user = userEvent.setup();
    renderAt(configurationPath("routes", MODEL_RULE.id));
    const inspector = await screen.findByRole("region", {
      name: "Configuration inspector",
    });
    expect(
      within(inspector).getByRole("heading", { name: MODEL_RULE.client_model }),
    ).toBeInTheDocument();
    expect(within(inspector).getByText("Priority 0")).toBeInTheDocument();
    expect(within(inspector).getByText(CHANNEL.name)).toBeInTheDocument();
    await user.click(
      within(inspector).getByRole("link", {
        name: new RegExp(MODEL.display_name),
      }),
    );
    await screen.findByRole("heading", { name: "Pricing" });
    expect(new URLSearchParams(window.location.search).get("selected")).toBe(
      MODEL.id,
    );
    await user.click(
      within(
        screen.getByRole("region", { name: "Configuration inspector" }),
      ).getByRole("link", { name: new RegExp(CHANNEL_GROUP.name) }),
    );
    await screen.findByText("Routes using this group");
    await user.click(
      within(
        screen.getByRole("region", { name: "Configuration inspector" }),
      ).getByRole("link", { name: new RegExp(MODEL_RULE.client_model) }),
    );
    expect(await screen.findByText("Request path")).toBeInTheDocument();
  });

  it("persists search, filter and selection in URLs and preserves context through saving", async () => {
    const user = userEvent.setup();
    server.use(
      http.put("/console/v1/models/:id", ({ request }) => {
        expect(request.headers.get("If-Match")).toBeTruthy();
        return HttpResponse.json({ id: MODEL.id, correlation_id: MODEL.id });
      }),
    );
    renderAt(`/admin/models?q=mini&facet=OpenAI&selected=${MODEL.id}`);
    const edit = await screen.findByRole("link", { name: "Edit model" });
    const returnTo = new URL(
      edit.getAttribute("href")!,
      window.location.origin,
    ).searchParams.get("returnTo")!;
    expect(new URLSearchParams(returnTo.split("?")[1]).get("q")).toBe("mini");
    await user.click(edit);
    await user.clear(await screen.findByLabelText("Display name"));
    await user.type(screen.getByLabelText("Display name"), "Updated display");
    await user.click(
      screen.getByRole("button", { name: "Save upstream model" }),
    );
    await waitFor(() =>
      expect(window.location.pathname + window.location.search).toBe(returnTo),
    );
  });

  it("prefills a new rule from its upstream model using only resource identifiers", async () => {
    const user = userEvent.setup();
    renderAt(configurationPath("models", MODEL.id));
    await user.click(
      await screen.findByRole("link", { name: "Create route for model" }),
    );
    expect(
      new URLSearchParams(window.location.search).get("upstreamModelId"),
    ).toBe(MODEL.id);
    expect(window.location.search).not.toContain("upstream_api_key");
    expect(
      await screen.findByRole("button", { name: "Create rule" }),
    ).toBeInTheDocument();
  });

  it("shows both format projections of a Codex pool without ordinary credential-copy actions", async () => {
    const responses = {
      ...CHANNEL_GROUP,
      id: "00000000-0000-0000-0000-000000000091",
      api_format: "open_ai_responses",
      connector_kind: "codex_oauth",
      connector_pool_id: "pool",
      name: "Shared Codex",
    };
    const images = {
      ...responses,
      id: "00000000-0000-0000-0000-000000000092",
      api_format: "open_ai_images",
      name: "Shared Codex Images",
      enabled: false,
    };
    server.use(
      http.get("/console/v1/routing/channel-groups", () =>
        HttpResponse.json([images, responses]),
      ),
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json([
          {
            ...CHANNEL,
            channel_group_id: responses.id,
            api_format: "open_ai_responses",
            provider_managed: true,
            connector_kind: "codex_oauth",
          },
        ]),
      ),
    );
    renderAt(configurationPath("supply", images.id));
    expect(
      await screen.findByRole("link", { name: "Manage shared credentials" }),
    ).toHaveAttribute("href", expect.stringContaining(responses.id));
    expect(screen.getByText("Group disabled")).toBeInTheDocument();
    const directory = screen.getByRole("region", {
      name: "Configuration directory",
    });
    expect(
      within(directory).getAllByRole("link", { name: /Shared Codex/ }),
    ).toHaveLength(1);
    expect(
      within(
        screen.getByRole("region", { name: "Configuration inspector" }),
      ).queryByRole("link", { name: "Copy" }),
    ).not.toBeInTheDocument();
  });

  it("does not replace a stale explicit selection with an unrelated resource", async () => {
    renderAt(configurationPath("routes", "deleted-id"));
    expect(
      await screen.findByText("Selection unavailable"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("link", { name: "Edit route" }),
    ).not.toBeInTheDocument();
  });

  it("filters attention cases and resets the selection rather than showing a hidden match", async () => {
    const user = userEvent.setup();
    server.use(
      http.get("/console/v1/models", () =>
        HttpResponse.json([
          MODEL,
          {
            ...MODEL,
            id: "orphan",
            display_name: "Unused model",
            source_model_id: "unused",
          },
        ]),
      ),
    );
    renderAt(configurationPath("models", MODEL.id));
    await user.click(
      await screen.findByRole("button", { name: /Needs attention/ }),
    );
    expect(new URLSearchParams(window.location.search).get("state")).toBe(
      "attention",
    );
    expect(new URLSearchParams(window.location.search).has("selected")).toBe(
      false,
    );
    const directory = screen.getByRole("region", {
      name: "Configuration directory",
    });
    expect(
      within(directory).queryByRole("link", {
        name: new RegExp(MODEL.display_name),
      }),
    ).not.toBeInTheDocument();
    expect(
      within(directory).getByRole("link", { name: /Unused model/ }),
    ).toBeInTheDocument();
  });

  it("paginates large directories and searches across pages", async () => {
    const user = userEvent.setup();
    server.use(
      http.get("/console/v1/models", () =>
        HttpResponse.json(
          Array.from({ length: 30 }, (_, i) => ({
            ...MODEL,
            id: `model-${i}`,
            display_name: `Model ${String(i).padStart(2, "0")}`,
            source_model_id: `upstream-${i}`,
          })),
        ),
      ),
    );
    renderAt("/admin/models");
    await user.click(await screen.findByRole("button", { name: "Next page" }));
    expect(new URLSearchParams(window.location.search).get("page")).toBe("2");
    await user.type(
      screen.getByRole("searchbox", { name: "Search configuration" }),
      "upstream-0",
    );
    expect(new URLSearchParams(window.location.search).has("page")).toBe(false);
    expect(screen.getByText(/1 results/)).toBeInTheDocument();
  });

  it("guards dirty detail forms on Cancel and relationship navigation", async () => {
    const user = userEvent.setup();
    renderAt(`/admin/models/${MODEL.id}`);
    await user.type(await screen.findByLabelText("Display name"), " draft");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    let dialog = screen.getByRole("alertdialog", {
      name: "Discard unsaved changes?",
    });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.getByLabelText("Display name")).toHaveValue(
      `${MODEL.display_name} draft`,
    );
    await user.click(screen.getByRole("link", { name: /Channel supply/ }));
    dialog = screen.getByRole("alertdialog", {
      name: "Discard unsaved changes?",
    });
    await user.click(
      within(dialog).getByRole("button", { name: "Discard changes" }),
    );
    await waitFor(() =>
      expect(window.location.pathname).toBe("/admin/routing/channels"),
    );
  });

  it("does not disguise an API error as an empty configuration", async () => {
    server.use(
      http.get("/console/v1/routing/channels", () =>
        HttpResponse.json({ error: "Supply unavailable" }, { status: 500 }),
      ),
    );
    renderAt("/admin/models");
    expect(await screen.findByText(/Supply unavailable/)).toBeInTheDocument();
    expect(
      screen.queryByText("No matching configuration"),
    ).not.toBeInTheDocument();
  });
});
