import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { AppProviders } from "@/app/providers";
import { MODEL } from "@/test/fixtures";
import { server } from "@/test/msw";
import { OnboardingPricing } from "./onboarding-pricing";

describe("onboarding pricing", () => {
  it("copies precise prices, units and advanced billing into a new identity, not the source model", async () => {
    const user = userEvent.setup();
    const onReview = vi.fn();
    const model = { ...MODEL, price_unit_tokens: 1001, output_unit_price: "1.000000000001", advanced_billing: {
      ...MODEL.advanced_billing, request_multipliers: [{ json_pointer: "/effort", value: "high", multiplier: "1.5" }],
    } };
    render(<AppProviders><OnboardingPricing models={[model]} onReview={onReview} onEdit={() => {}} catalogRevision={0} /></AppProviders>);
    await user.type(await screen.findByLabelText("Client model id"), "next-model");
    screen.getByRole("combobox", { name: "Price source" }).focus();
    await user.keyboard("{ArrowDown}");
    await user.click(await screen.findByRole("option", { name: "Copy local pricing" }));
    screen.getByRole("combobox", { name: "Copy pricing from model" }).focus();
    await user.keyboard("{ArrowDown}");
    await user.click(await screen.findByRole("option", { name: MODEL.source_model_id }));
    await user.click(screen.getByRole("button", { name: "Review routing candidates" }));
    expect(onReview.mock.lastCall?.[0].model).toEqual({ create: expect.objectContaining({ source_model_id: "next-model", price_unit_tokens: 1001,
      output_unit_price: model.output_unit_price, advanced_billing: model.advanced_billing }) });
  });

  it("uses only the selected exact provider quote and invalidates its token on reload", async () => {
    const user = userEvent.setup();
    const onReview = vi.fn();
    const base = { ...MODEL, model_id: "next-model", action: "import" };
    server.use(http.post("/console/v1/catalog/models/sync/preview", () => HttpResponse.json({ models: [
      { ...base, model_id: "next-model-v2", provider_id: "other", provider_name: "Not exact", preview_token: "1".repeat(64) },
      { ...base, provider_id: "north", provider_name: "North", preview_token: "2".repeat(64) },
      { ...base, provider_id: "south", provider_name: "South", preview_token: "3".repeat(64) },
    ] })));
    const props = { models: [], onReview, onEdit: () => {} };
    const rendered = render(<AppProviders><OnboardingPricing {...props} catalogRevision={0} /></AppProviders>);
    await user.type(await screen.findByLabelText("Client model id"), "next-model");
    screen.getByRole("combobox", { name: "Price source" }).focus();
    await user.keyboard("{ArrowDown}");
    await user.click(await screen.findByRole("option", { name: "Import catalog pricing" }));
    await user.click(screen.getByRole("button", { name: "Find exact catalog model" }));
    (await screen.findByRole("combobox", {name: "Catalog entry"})).focus();
    await user.keyboard("{ArrowDown}");
    const south = await screen.findByRole("option", { name: /South/ });
    expect(screen.queryByRole("option", { name: /Not exact/ })).not.toBeInTheDocument();
    await user.click(south);
    await user.click(screen.getByRole("button", { name: "Review routing candidates" }));
    expect(onReview.mock.lastCall?.[0].model).toEqual({ catalog: { model_id: "next-model", provider_id: "south", preview_token: "3".repeat(64) } });
    rendered.rerender(<AppProviders><OnboardingPricing {...props} catalogRevision={1} /></AppProviders>);
    await user.click(screen.getByRole("button", { name: "Review routing candidates" }));
    expect(onReview).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("alert")).toHaveTextContent("confirm its pricing");
  });
});
