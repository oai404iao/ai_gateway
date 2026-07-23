import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL, CHANNEL_DETAIL } from "@/test/fixtures";
import type { ChannelInput } from "@/api/types";

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

describe("ChannelDetailPage", () => {
  it("loads and resubmits the stored upstream key and override document", async () => {
    seedAuthenticatedSession();
    let submitted: ChannelInput | undefined;
    server.use(
      http.put("/console/v1/routing/channels/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelInput;
        return HttpResponse.json({
          id: CHANNEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channels/${CHANNEL.id}`);

    expect(await screen.findByDisplayValue(CHANNEL.name)).toBeInTheDocument();
    expect(screen.getByLabelText(/upstream api key/i)).toHaveValue(
      CHANNEL_DETAIL.upstream_api_key,
    );
    await user.click(screen.getByRole("tab", { name: /json configuration/i }));
    expect(screen.getByLabelText(/transform document json/i)).toHaveValue(
      JSON.stringify(CHANNEL_DETAIL.override_document, null, 2),
    );
    await user.click(screen.getByRole("button", { name: /save channel/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.override_document).toEqual(CHANNEL_DETAIL.override_document);
    expect(submitted?.upstream_api_key).toBe(CHANNEL_DETAIL.upstream_api_key);
    expect(submitted?.status_statistics_enabled).toBe(true);
    expect(submitted?.auto_disable_allowed).toBe(true);
    expect(submitted?.billing_multiplier).toBe(CHANNEL.billing_multiplier);
    expect(submitted?.test_model).toBe(CHANNEL.test_model);
  });

  it("explains a routing dependency rejection instead of showing an opaque error", async () => {
    seedAuthenticatedSession();
    server.use(
      http.put("/console/v1/routing/channels/:id", () =>
        HttpResponse.json({ error: "routing_dependency_invalid" }, { status: 422 }),
      ),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channels/${CHANNEL.id}`);

    expect(await screen.findByDisplayValue(CHANNEL.name)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /save channel/i }));

    expect(
      await screen.findByText(/would make the routing configuration invalid/i),
    ).toBeInTheDocument();
  });

  it("blocks disabling the only eligible channel before sending the update", async () => {
    seedAuthenticatedSession();
    let putHit = false;
    server.use(
      http.put("/console/v1/routing/channels/:id", () => {
        putHit = true;
        return HttpResponse.json({
          id: CHANNEL.id,
          correlation_id: "44444444-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channels/${CHANNEL.id}`);

    expect(await screen.findByDisplayValue(CHANNEL.name)).toBeInTheDocument();
    await user.click(screen.getByRole("switch", { name: /^enabled$/i }));
    await user.click(screen.getByRole("button", { name: /save channel/i }));

    expect(
      await screen.findByText(/would make the routing configuration invalid/i),
    ).toBeInTheDocument();
    expect(putHit).toBe(false);
  });

  it("adds and removes available upstream models as a token list", async () => {
    seedAuthenticatedSession();
    let submitted: ChannelInput | undefined;
    server.use(
      http.put("/console/v1/routing/channels/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelInput;
        return HttpResponse.json({
          id: CHANNEL.id,
          correlation_id: "55555555-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channels/${CHANNEL.id}`);

    const models = await screen.findByLabelText(/available upstream models/i);
    await user.type(models, "anthropic/claude-sonnet-4");
    await user.keyboard("{Enter}");
    expect(screen.getByText("anthropic/claude-sonnet-4")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Remove anthropic/claude-sonnet-4" }),
    );
    expect(screen.queryByText("anthropic/claude-sonnet-4")).not.toBeInTheDocument();

    await user.type(models, "openai/gpt-4.1");
    await user.keyboard("{Enter}");
    await user.click(screen.getByRole("button", { name: /save channel/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.available_models).toEqual([
      ...CHANNEL.available_models,
      "openai/gpt-4.1",
    ]);
  });
});
