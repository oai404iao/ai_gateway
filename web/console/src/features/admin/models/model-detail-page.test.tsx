import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  it("initializes copy mode when navigating from the existing detail route", async () => {
    seedAuthenticatedSession();
    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);

    await user.click(
      await screen.findByRole("link", {
        name: `Copy ${MODEL.display_name}`,
      }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/models/new");
    });
    expect(await screen.findByLabelText("Client model id")).toHaveValue("");
    expect(screen.getByLabelText("Display name")).toHaveValue(
      `${MODEL.display_name} copy`,
    );
  });

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

    expect(await screen.findByLabelText("Client model id")).toBeDisabled();
    expect(
      screen.getByText(
        "The client model ID cannot change after a model rule is created.",
      ),
    ).toBeInTheDocument();
    const editor = await screen.findByLabelText(/advanced billing/i);
    expect(
      screen.getByRole("link", { name: /configure pricing/i }),
    ).toBeInTheDocument();
    fireEvent.change(editor, { target: { value: JSON.stringify(advancedBilling) } });
    await user.click(screen.getByRole("button", { name: /save client model/i }));

    await waitFor(() => {
      expect(submitted).toBeDefined();
    });
    expect(submitted?.advanced_billing).toEqual(advancedBilling);
  });

  it("permanently deletes a pricing model after confirming its effects", async () => {
    seedAuthenticatedSession();
    let ifMatch: string | null = null;
    server.use(
      http.delete("/console/v1/models/:id", ({ request }) => {
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000001",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}`);

    await user.click(
      await screen.findByRole("button", { name: "Delete client model" }),
    );
    const confirmation = screen.getByRole("alertdialog", {
      name: "Delete client model?",
    });
    expect(
      within(confirmation).getByText(/clears scheduled test pricing references/i),
    ).toBeInTheDocument();
    await user.click(
      within(confirmation).getByRole("button", {
        name: "Delete client model",
      }),
    );

    await waitFor(() => {
      expect(window.location.pathname).toBe("/admin/models");
    });
    expect(ifMatch).toBe(`"${MODEL.updated_at}"`);
    expect(await screen.findByText("Client model deleted")).toBeInTheDocument();
  });
});
