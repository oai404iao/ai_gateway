import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
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

describe("ModelPricingPage", () => {
  it("calculates exact prices, fills the DeepSeek preset, and preserves model policy", async () => {
    seedAuthenticatedSession();
    let submitted: ModelInput | undefined;
    let ifMatch: string | null = null;
    const longContextTiers = [
      {
        input_tokens_threshold: 128_000,
        input_unit_price: "0.3",
        cached_input_unit_price: "0.15",
        cache_write_unit_price: "0.6",
        output_unit_price: "0.9",
      },
    ];
    const requestMultipliers = [
      {
        json_pointer: "/reasoning/effort",
        value: "high",
        multiplier: "2",
      },
    ];
    server.use(
      http.get("/console/v1/models/:id", () =>
        HttpResponse.json(
          {
            ...MODEL,
            advanced_billing: {
              long_context_tiers: longContextTiers,
              request_multipliers: requestMultipliers,
            },
          },
          { headers: { ETag: `"${MODEL.updated_at}"` } },
        ),
      ),
      http.put("/console/v1/models/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelInput;
        ifMatch = request.headers.get("if-match");
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000001",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    const multiplier = await screen.findByLabelText(/calculator multiplier/i);
    await user.clear(multiplier);
    await user.type(multiplier, "1.2");
    await user.click(screen.getByRole("button", { name: /calculate and fill base prices/i }));

    expect(screen.getByLabelText(/^input unit price/i)).toHaveValue("0.18");
    expect(screen.getByLabelText(/^cached input unit price/i)).toHaveValue("0.09");
    expect(screen.getByLabelText(/^cache write unit price/i)).toHaveValue("0.36");
    expect(screen.getByLabelText(/^output unit price/i)).toHaveValue("0.72");

    await user.click(screen.getByRole("button", { name: /use deepseek preset/i }));
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(ifMatch).toBe(`"${MODEL.updated_at}"`);
    expect(submitted).toMatchObject({
      source_model_id: MODEL.source_model_id,
      display_name: MODEL.display_name,
      input_unit_price: "0.18",
      cached_input_unit_price: "0.09",
      cache_write_unit_price: "0.36",
      output_unit_price: "0.72",
      advanced_billing: {
        long_context_tiers: longContextTiers,
        request_multipliers: requestMultipliers,
        time_multipliers: [
          {
            start_time: "01:00",
            end_time: "04:00",
            multiplier: "2",
            weekdays: [
              "monday",
              "tuesday",
              "wednesday",
              "thursday",
              "friday",
              "saturday",
              "sunday",
            ],
          },
          {
            start_time: "06:00",
            end_time: "10:00",
            multiplier: "2",
            weekdays: [
              "monday",
              "tuesday",
              "wednesday",
              "thursday",
              "friday",
              "saturday",
              "sunday",
            ],
          },
        ],
      },
    });
    expect(submitted).not.toHaveProperty("source_payload");
  });

  it("rejects overlapping UTC windows before sending an update", async () => {
    seedAuthenticatedSession();
    let updateCount = 0;
    server.use(
      http.put("/console/v1/models/:id", () => {
        updateCount += 1;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000002",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    await screen.findByText(/^base prices$/i, {
      selector: '[data-slot="card-title"]',
    });
    await user.click(screen.getByRole("button", { name: /add window/i }));
    await user.click(screen.getByRole("button", { name: /add window/i }));
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));

    expect(
      await screen.findByText(/utc price windows cannot overlap on the selected weekdays/i),
    ).toBeInTheDocument();
    expect(updateCount).toBe(0);
  });

  it("preserves the exact effective timestamp when only pricing rules change", async () => {
    seedAuthenticatedSession();
    const preciseTimestamp = "2026-01-01T03:12:49.958123Z";
    let submitted: ModelInput | undefined;
    server.use(
      http.get("/console/v1/models/:id", () =>
        HttpResponse.json(
          { ...MODEL, price_effective_at: preciseTimestamp },
          { headers: { ETag: `"${MODEL.updated_at}"` } },
        ),
      ),
      http.put("/console/v1/models/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelInput;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000003",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    await screen.findByText(/^base prices$/i, {
      selector: '[data-slot="card-title"]',
    });
    await user.click(screen.getByRole("button", { name: /use deepseek preset/i }));
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.price_effective_at).toBe(preciseTimestamp);
  });

  it("refetches a conflicted model and uses the refreshed ETag", async () => {
    seedAuthenticatedSession();
    let getCount = 0;
    const ifMatches: Array<string | null> = [];
    server.use(
      http.get("/console/v1/models/:id", () => {
        getCount += 1;
        const refreshed = getCount > 1;
        return HttpResponse.json(
          {
            ...MODEL,
            input_unit_price: refreshed ? "9" : MODEL.input_unit_price,
          },
          { headers: { ETag: refreshed ? `"v2"` : `"v1"` } },
        );
      }),
      http.put("/console/v1/models/:id", ({ request }) => {
        ifMatches.push(request.headers.get("if-match"));
        if (ifMatches.length === 1) {
          return HttpResponse.json(
            { error: "Console operation rejected" },
            { status: 409 },
          );
        }
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000004",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    expect(await screen.findByLabelText(/^input unit price/i)).toHaveValue(
      MODEL.input_unit_price,
    );
    await user.clear(screen.getByLabelText(/^input unit price/i));
    await user.type(screen.getByLabelText(/^input unit price/i), "2");
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));
    await waitFor(() =>
      expect(screen.getByLabelText(/^input unit price/i)).toHaveValue("9"),
    );

    await user.clear(screen.getByLabelText(/^input unit price/i));
    await user.type(screen.getByLabelText(/^input unit price/i), "8");
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));
    await waitFor(() => expect(ifMatches).toEqual(['"v1"', '"v2"']));
  });

  it("keeps a saved draft visible and disables saving when the latest ETag cannot reload", async () => {
    seedAuthenticatedSession();
    let failReload = false;
    server.use(
      http.get("/console/v1/models/:id", () => {
        if (failReload) {
          return HttpResponse.json(
            { error: "Temporary reload failure" },
            { status: 503 },
          );
        }
        return HttpResponse.json(
          { ...MODEL, input_unit_price: "2" },
          { headers: { ETag: `"v2"` } },
        );
      }),
      http.put("/console/v1/models/:id", () => {
        failReload = true;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000005",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    const inputPrice = await screen.findByLabelText(/^input unit price/i);
    await user.clear(inputPrice);
    await user.type(inputPrice, "3");
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));

    const reload = await screen.findByRole("button", {
      name: /reload latest model/i,
    });
    expect(inputPrice).toHaveValue("3");
    expect(screen.getByRole("button", { name: /save model pricing/i })).toBeDisabled();

    failReload = false;
    await user.click(reload);
    await waitFor(() => expect(inputPrice).toHaveValue("2"));
    expect(
      screen.queryByRole("button", { name: /reload latest model/i }),
    ).not.toBeInTheDocument();
  });

  it("allows equal clock windows on different weekdays and submits weekday selections", async () => {
    seedAuthenticatedSession();
    let submitted: ModelInput | undefined;
    server.use(
      http.put("/console/v1/models/:id", async ({ request }) => {
        submitted = (await request.json()) as ModelInput;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000006",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    await screen.findByText(/^base prices$/i, {
      selector: '[data-slot="card-title"]',
    });
    await user.click(screen.getByRole("button", { name: /add window/i }));
    await user.click(screen.getByRole("button", { name: /weekdays/i }));
    await user.click(screen.getByRole("button", { name: /add window/i }));
    await user.click(screen.getAllByRole("button", { name: /weekend/i }).at(-1)!);
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));

    await waitFor(() => expect(submitted).toBeDefined());
    expect(submitted?.advanced_billing?.time_multipliers).toMatchObject([
      {
        weekdays: ["monday", "tuesday", "wednesday", "thursday", "friday"],
        start_time: "00:00",
        end_time: "01:00",
      },
      {
        weekdays: ["saturday", "sunday"],
        start_time: "00:00",
        end_time: "01:00",
      },
    ]);
  });

  it("defaults legacy windows to every day and requires at least one weekday", async () => {
    seedAuthenticatedSession();
    let updateCount = 0;
    server.use(
      http.get("/console/v1/models/:id", () =>
        HttpResponse.json(
          {
            ...MODEL,
            advanced_billing: {
              long_context_tiers: [],
              request_multipliers: [],
              time_multipliers: [
                {
                  label: "Legacy peak",
                  start_time: "01:00",
                  end_time: "04:00",
                  multiplier: "2",
                },
              ],
            },
          },
          { headers: { ETag: `"${MODEL.updated_at}"` } },
        ),
      ),
      http.put("/console/v1/models/:id", () => {
        updateCount += 1;
        return HttpResponse.json({
          id: MODEL.id,
          correlation_id: "33333333-0000-0000-0000-000000000007",
        });
      }),
    );

    const user = userEvent.setup();
    renderAppAt(`/admin/models/${MODEL.id}/pricing`);

    for (const weekday of [
      "Monday",
      "Tuesday",
      "Wednesday",
      "Thursday",
      "Friday",
      "Saturday",
      "Sunday",
    ]) {
      const toggle = await screen.findByRole("button", { name: weekday });
      expect(toggle).toHaveAttribute("aria-pressed", "true");
      await user.click(toggle);
    }
    await user.click(screen.getByRole("button", { name: /save model pricing/i }));

    expect(await screen.findByText(/select at least one utc weekday/i)).toBeInTheDocument();
    expect(updateCount).toBe(0);
  });
});
