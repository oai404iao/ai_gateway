import { describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import type { ModelSyncPreview, ModelSyncPreviewModel } from "@/api/types";
import { AppProviders } from "@/app/providers";
import { CatalogPage } from "./catalog-page";
import { server, seedAuthenticatedSession } from "@/test/msw";

const model: ModelSyncPreviewModel = {
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
    request_multipliers: [
      { json_pointer: "/service_tier", value: "priority", multiplier: "2" },
    ],
  },
  action: "price_update",
  preview_token: "a".repeat(64),
};

function previewResponse(row = model) {
  return HttpResponse.json({
    fetched_at: "2026-07-20T00:00:00.000Z",
    models: [row],
    excluded_missing_prices: 0,
    excluded_invalid_models: 0,
    excluded_oversized_metadata: 0,
  } satisfies ModelSyncPreview);
}

describe("CatalogPage", () => {
  it("allows a selected existing local model to refresh its catalog price", async () => {
    seedAuthenticatedSession();
    let applied: unknown;
    server.use(
      http.post("/console/v1/catalog/models/sync/preview", () => previewResponse()),
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
    const modelRow = await screen.findByRole("row", { name: /GPT-4o mini/ });
    expect(screen.getByRole("columnheader", { name: "Request multipliers" })).toBeInTheDocument();
    expect(within(modelRow).getAllByRole("cell")[5]).toHaveTextContent("1");
    await user.click(
      await screen.findByRole("checkbox", { name: "Select openai/gpt-4o-mini" }),
    );
    await user.click(screen.getByRole("button", { name: "Apply selected (1)" }));

    await waitFor(() => {
      expect(applied).toEqual({
        selections: [{ provider_id: "openai", model_id: model.model_id, preview_token: model.preview_token }],
      });
    });
  });

  it("discards a conflicting preview and requires a new selection before applying again", async () => {
    seedAuthenticatedSession();
    const changed = { ...model, output_unit_price: "0.60000001", preview_token: "b".repeat(64) };
    const applied: unknown[] = [];
    let previewCount = 0;
    server.use(
      http.post("/console/v1/catalog/models/sync/preview", () =>
        previewResponse(++previewCount === 1 ? model : changed),
      ),
      http.post("/console/v1/catalog/models/import", async ({ request }) => {
        applied.push(await request.json());
        if (applied.length === 1) return HttpResponse.json({ error: "conflict" }, { status: 409 });
        return HttpResponse.json({
          model_count: 1, imported_count: 0, updated_count: 1,
          correlation_id: "00000000-0000-0000-0000-000000000042",
        });
      }),
    );
    const user = userEvent.setup();
    render(<AppProviders><CatalogPage /></AppProviders>);
    const fetchButton = screen.getByRole("button", { name: "Fetch preview" });
    const selectRow = () => screen.findByRole("checkbox", { name: `Select ${model.model_id}` });
    const applySelected = () => screen.getByRole("button", { name: "Apply selected (1)" });

    await user.click(fetchButton);
    await user.click(await screen.findByRole("button", { name: "Billing details" }));
    expect(screen.getByText("USD per million tokens")).toBeVisible();
    expect(await selectRow()).not.toBeChecked();
    await user.click(await selectRow());
    await user.click(applySelected());
    expect(await screen.findByText(/Nothing was applied/)).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Apply selected (0)" })).toBeDisabled();

    await user.click(fetchButton);
    const refreshed = await selectRow();
    await user.click(screen.getByRole("button", { name: "Billing details" }));
    expect(screen.getByText(changed.output_unit_price)).toBeVisible();
    expect(screen.getByText(/\/service_tier = "priority"/)).toBeVisible();
    expect(refreshed).not.toBeChecked();
    expect(applied).toHaveLength(1);
    await user.click(refreshed);
    await user.click(applySelected());
    await waitFor(() => expect(applied).toEqual([model, changed].map((row) => ({
      selections: [{ provider_id: row.provider_id, model_id: row.model_id, preview_token: row.preview_token }],
    }))));
  });
});
