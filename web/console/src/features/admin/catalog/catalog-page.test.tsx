import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { CatalogPage } from "./catalog-page";
import { server, seedAuthenticatedSession } from "@/test/msw";

describe("CatalogPage", () => {
  it("allows a selected existing local model to refresh its catalog price", async () => {
    seedAuthenticatedSession();
    let applied: unknown;
    server.use(
      http.post("/console/v1/catalog/models/sync/preview", () =>
        HttpResponse.json({
          fetched_at: "2026-07-20T00:00:00.000Z",
          models: [
            {
              provider_id: "openai",
              provider_name: "OpenAI",
              model_id: "openai/gpt-4o-mini",
              display_name: "GPT-4o mini",
              input_unit_price: "0.15",
              cached_input_unit_price: "0.075",
              cache_write_unit_price: "0.3",
              output_unit_price: "0.6",
              advanced_billing: {
                long_context_tiers: [
                  {
                    input_tokens_threshold: 200_000,
                    input_unit_price: "0.3",
                    cached_input_unit_price: "0.15",
                    cache_write_unit_price: "0.6",
                    output_unit_price: "0.9",
                  },
                ],
                request_multipliers: [],
              },
              action: "price_update",
            },
          ],
          excluded_missing_prices: 0,
          excluded_invalid_models: 0,
          excluded_oversized_metadata: 0,
        }),
      ),
      http.post("/console/v1/catalog/models/import", async ({ request }) => {
        applied = await request.json();
        return HttpResponse.json({
          model_count: 1,
          imported_count: 0,
          updated_count: 1,
          correlation_id: "00000000-0000-0000-0000-000000000041",
        });
      }),
    );

    const user = userEvent.setup();
    render(
      <AppProviders>
        <CatalogPage />
      </AppProviders>,
    );

    expect(screen.getByRole("heading", { name: "Price sync" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Fetch preview" }));
    await user.click(
      await screen.findByRole("checkbox", { name: "Select openai/gpt-4o-mini" }),
    );
    expect(screen.getByText("1")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Apply selected (1)" }));

    await waitFor(() => {
      expect(applied).toEqual({
        selections: [{ provider_id: "openai", model_id: "openai/gpt-4o-mini" }],
      });
    });
  });
});
