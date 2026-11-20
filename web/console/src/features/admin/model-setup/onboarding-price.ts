import type { AdvancedBilling, ControlPlaneModel, ModelInput, ModelOnboardingInput } from "@/api/types";

export const priceFields = [
  ["input_unit_price", "Input unit price"], ["cached_input_unit_price", "Cached input unit price"],
  ["cache_write_unit_price", "Cache write unit price"], ["output_unit_price", "Output unit price"],
] as const;
export type Rates = Pick<ModelInput, typeof priceFields[number][0]>;
export type Quote = Rates & { price_unit_tokens: number; advanced_billing?: AdvancedBilling };
export type PricedSelection = { name: string; source?: string; quote: Quote; model: ModelOnboardingInput["model"] };
export const existingSelection = (model: ControlPlaneModel): PricedSelection => ({
  name: model.source_model_id, quote: model, model: { existing: { id: model.id, updated_at: model.updated_at } },
});
