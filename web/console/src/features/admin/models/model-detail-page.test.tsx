import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { BrowserRouter } from "react-router";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import type { ModelInput } from "@/api/types";
import { MODEL } from "@/test/fixtures";
import { server, seedAuthenticatedSession } from "@/test/msw";

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

describe("ModelDetailPage", () => {
  it("loads and submits the model-level advanced billing policy", async () => {
    seedAuthenticatedSession();
    let submitted: ModelInput | undefined;
    const advancedBilling = {
      long_context_tiers: [
        {
          input_tokens_threshold: 128_000,
          input_unit_price: "0.3",
          cached_input_unit_price: "0.15",
          cache_write_unit_price: "0.6",
          output_unit_price: "0.9",
        },
      ],
      request_multipliers: [
        {
          json_pointer: "/reasoning/effort",
          value: "high",
          multiplier: "2",
        },
      ],
    };
    server.use(
      http.put("/console/v1/models/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelInput;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000000",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);

    const editor = await screen.findByLabelText(/advanced billing/i);
    fireEvent.change(editor, { target: { value: JSON.stringify(advancedBilling) } });
    await user.click(screen.getByRole("button", { name: /save upstream model/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.advanced_billing).toEqual(advancedBilling);
  });
});
