import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { server, seedAuthenticatedSession } from "@/test/msw";
import { CHANNEL, CHANNEL_DETAIL } from "@/test/fixtures";
import type { ChannelInput, ChannelModelDiscoveryInput } from "@/api/types";

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

  it("fetches upstream models from the channel draft and applies the selection", async () => {
    seedAuthenticatedSession();
    let discoveryInput: ChannelModelDiscoveryInput | undefined;
    let submitted: ChannelInput | undefined;
    server.use(
      http.post(
        "/console/v1/routing/channels/models/discover",
        async ({ request }) => {
          discoveryInput = (await request.json()) as ChannelModelDiscoveryInput;
          return HttpResponse.json({
            models: [
              CHANNEL.available_models[0],
              "openai/gpt-4.1",
              "anthropic/claude-sonnet-4",
            ],
          });
        },
      ),
      http.put("/console/v1/routing/channels/:id", async ({ request }) => {
        submitted = (await request.json()) as ChannelInput;
        return HttpResponse.json({
          id: CHANNEL.id,
          correlation_id: "66666666-0000-0000-0000-000000000000",
        });
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/routing/channels/${CHANNEL.id}`);

    expect(await screen.findByDisplayValue(CHANNEL.name)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /fetch models/i }));

    expect(
      await screen.findByRole("heading", { name: /select upstream models/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", {
        name: `Select ${CHANNEL.available_models[0]}`,
      }),
    ).toBeChecked();
    await user.click(
      screen.getByRole("checkbox", { name: "Select openai/gpt-4.1" }),
    );
    await user.click(screen.getByRole("button", { name: /apply selection/i }));

    expect(screen.getByText("openai/gpt-4.1")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /save channel/i }));

    await waitFor(() => {
      expect(discoveryInput).toBeDefined();
      expect(submitted).toBeDefined();
    });
    expect(discoveryInput).toMatchObject({
      api_format: CHANNEL.api_format,
      base_url: CHANNEL.base_url,
      upstream_auth_kind: CHANNEL.upstream_auth_kind,
      upstream_api_key: CHANNEL_DETAIL.upstream_api_key,
      override_document: CHANNEL_DETAIL.override_document,
    });
    expect(submitted?.available_models).toEqual([
      ...CHANNEL.available_models,
      "openai/gpt-4.1",
    ]);
  });
});
