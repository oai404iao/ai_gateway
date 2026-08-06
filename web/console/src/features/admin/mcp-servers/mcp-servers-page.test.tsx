import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { seedAuthenticatedSession } from "@/test/msw";

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

describe("McpServersPage", () => {
  it("lists stateless search and image endpoints with their fixed tools", async () => {
    seedAuthenticatedSession();
    renderAppAt("/admin/mcp-servers");

    expect(
      await screen.findByRole("heading", { name: "MCP Servers" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("/mcp/research")).toBeInTheDocument();
    expect(screen.getByText("web.run")).toBeInTheDocument();
    expect(screen.getByText("/mcp/studio")).toBeInTheDocument();
    expect(screen.getByText("image_gen.imagegen")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "New MCP server" }),
    ).toBeInTheDocument();
  });
});
