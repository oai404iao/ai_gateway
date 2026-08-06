import { beforeEach, describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import type { McpServerCreateInput, McpServerInput } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import {
  IMAGE_MCP_SERVER,
  IMAGE_MODEL_RULE,
  SEARCH_MCP_SERVER,
  SEARCH_MODEL_RULE,
} from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

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

describe("McpServerDetailPage", () => {
  beforeEach(() => {
    server.use(
      http.get("/console/v1/routing/model-rules", () =>
        HttpResponse.json([SEARCH_MODEL_RULE, IMAGE_MODEL_RULE]),
      ),
    );
  });

  it("creates a web search endpoint with normalized typed settings", async () => {
    seedAuthenticatedSession();
    let submitted: McpServerCreateInput | undefined;
    server.use(
      http.post("/console/v1/mcp-servers", async ({ request }) => {
        submitted = (await request.json()) as McpServerCreateInput;
        return HttpResponse.json(
          {
            id: SEARCH_MCP_SERVER.id,
            correlation_id: "99999999-0000-0000-0000-000000000130",
          },
          { status: 201 },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/mcp-servers/new");

    await user.type(await screen.findByLabelText("Endpoint slug"), "research-2");
    await user.type(screen.getByLabelText("Name"), "Research two");
    await user.type(
      screen.getByLabelText("Description"),
      "Search approved sources.",
    );
    await user.click(screen.getByRole("combobox", { name: "Model rule" }));
    await user.click(
      await screen.findByRole("option", {
        name: `${SEARCH_MODEL_RULE.client_model} → ${SEARCH_MODEL_RULE.upstream_model}`,
      }),
    );
    await user.type(
      screen.getByLabelText("Allowed domains"),
      "Example.com\ndocs.example.com",
    );
    await user.type(
      screen.getByLabelText("Blocked domains"),
      "ads.example.com",
    );
    await user.click(
      screen.getByRole("button", { name: "Create MCP server" }),
    );

    await waitFor(() => {
      expect(submitted).toEqual({
        slug: "research-2",
        kind: "web_search",
        name: "Research two",
        description: "Search approved sources.",
        model_rule_id: SEARCH_MODEL_RULE.id,
        settings: {
          external_web_access: "live",
          search_context_size: "medium",
          allowed_domains: ["example.com", "docs.example.com"],
          blocked_domains: ["ads.example.com"],
          max_output_tokens: {
            short: 1_000,
            medium: 3_000,
            long: 6_000,
          },
        },
        enabled: true,
      });
    });
  });

  it("updates image defaults with If-Match while preserving immutable identity", async () => {
    seedAuthenticatedSession();
    let submitted: McpServerInput | undefined;
    let ifMatch: string | null = null;
    server.use(
      http.put("/console/v1/mcp-servers/:id", async ({ request, params }) => {
        submitted = (await request.json()) as McpServerInput;
        ifMatch = request.headers.get("If-Match");
        return HttpResponse.json({
          id: String(params.id),
          correlation_id: "99999999-0000-0000-0000-000000000131",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/mcp-servers/${IMAGE_MCP_SERVER.id}`);

    const name = await screen.findByDisplayValue(IMAGE_MCP_SERVER.name);
    expect(screen.queryByLabelText("Endpoint slug")).not.toBeInTheDocument();
    expect(screen.getByText(`/mcp/${IMAGE_MCP_SERVER.slug}`)).toBeInTheDocument();

    await user.clear(name);
    await user.type(name, "Updated image studio");
    await user.click(screen.getByRole("combobox", { name: "Background" }));
    await user.click(await screen.findByRole("option", { name: "Transparent" }));
    await user.click(screen.getByRole("combobox", { name: "Quality" }));
    await user.click(await screen.findByRole("option", { name: "Medium" }));
    const size = screen.getByLabelText("Size");
    await user.clear(size);
    await user.type(size, "2048x1024");
    await user.click(screen.getByRole("button", { name: "Save MCP server" }));

    await waitFor(() => {
      expect(submitted).toEqual({
        name: "Updated image studio",
        description: IMAGE_MCP_SERVER.description,
        model_rule_id: IMAGE_MODEL_RULE.id,
        settings: {
          background: "transparent",
          quality: "medium",
          size: "2048x1024",
        },
        enabled: true,
      });
    });
    expect(ifMatch).toBe(`"${IMAGE_MCP_SERVER.updated_at}"`);
  });

  it("soft-deletes an endpoint with If-Match after warning about slug reuse", async () => {
    seedAuthenticatedSession();
    let ifMatch: string | null = null;
    server.use(
      http.delete("/console/v1/mcp-servers/:id", ({ request, params }) => {
        ifMatch = request.headers.get("If-Match");
        return HttpResponse.json({
          id: String(params.id),
          correlation_id: "99999999-0000-0000-0000-000000000132",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/mcp-servers/${SEARCH_MCP_SERVER.id}`);

    await screen.findByDisplayValue(SEARCH_MCP_SERVER.name);
    expect(screen.getByText("Endpoint slug remains reserved")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Delete MCP server" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(`/mcp/${SEARCH_MCP_SERVER.slug}`);
    await user.click(
      screen.getAllByRole("button", { name: "Delete MCP server" }).at(-1)!,
    );

    await waitFor(() => {
      expect(ifMatch).toBe(`"${SEARCH_MCP_SERVER.updated_at}"`);
    });
  });
});
