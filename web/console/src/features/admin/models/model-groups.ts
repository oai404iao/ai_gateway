import type { ControlPlaneModel } from "@/api/types";

export interface ModelProviderGroup {
  provider: string;
  models: ControlPlaneModel[];
}

export function groupModelsByProvider(
  models: readonly ControlPlaneModel[],
  fallbackProvider: string,
): ModelProviderGroup[] {
  const grouped = new Map<string, ControlPlaneModel[]>();
  for (const model of models) {
    const provider = model.provider_name?.trim() || fallbackProvider;
    grouped.set(provider, [...(grouped.get(provider) ?? []), model]);
  }

  return [...grouped.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([provider, providerModels]) => ({
      provider,
      models: providerModels.sort((left, right) => {
        const displayOrder = left.display_name.localeCompare(right.display_name);
        return displayOrder || left.source_model_id.localeCompare(right.source_model_id);
      }),
    }));
}
