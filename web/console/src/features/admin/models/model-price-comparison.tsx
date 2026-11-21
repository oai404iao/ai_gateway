import { useId, useState } from "react";
import { ChevronDown } from "lucide-react";
import type { ControlPlaneModel } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useModels } from "@/features/admin/api";
import { isNonNegativeDecimal } from "@/lib/decimal";
import { modelLabel } from "./model-label";

const buckets = [
  ["input_unit_price", "Uncached input"],
  ["cached_input_unit_price", "Cached input"],
  ["cache_write_unit_price", "Cache write"],
  ["output_unit_price", "Output"],
] as const;
type BaseRates = Pick<ControlPlaneModel, (typeof buckets)[number][0] | "price_unit_tokens">;

export function ModelPriceComparison({ model, draft }: { model: ControlPlaneModel; draft: BaseRates }) {
  const { t, locale } = useI18n();
  const models = useModels();
  const [expanded, setExpanded] = useState(false);
  const panelId = useId();
  const [otherId, setOtherId] = useState("");
  const [workload, setWorkload] = useState(["10000", "0", "0", "2000"]);
  const options = [model, ...(models.data ?? []).filter((item) => item.id !== model.id)];
  const comparison = options.find((item) => item.id === otherId) ?? model;
  const usd = new Intl.NumberFormat(locale, { style: "currency", currency: "USD", maximumFractionDigits: 9 });
  const validCount = (value: string) => /^\d+$/.test(value) && Number.isSafeInteger(Number(value));
  const estimate = (rates: BaseRates) => {
    if (!workload.every(validCount) || !Number.isSafeInteger(rates.price_unit_tokens) || rates.price_unit_tokens <= 0 || !buckets.every(([key]) => isNonNegativeDecimal(rates[key]))) return null;
    return buckets.map(([key], i) => Number(rates[key]) * Number(workload[i]) / rates.price_unit_tokens);
  };
  const prices = [estimate(draft), estimate(comparison)];

  return <section aria-label={t("Request cost comparison")} className="border-y">
    <Button type="button" variant="ghost" size="sm" className="w-full justify-between lg:hidden" aria-expanded={expanded} aria-controls={panelId} onClick={() => setExpanded(!expanded)}>
      {t("Request cost comparison")}<ChevronDown data-icon="inline-end" />
    </Button>
    <div id={panelId} className={cn(!expanded && "hidden lg:block")}>
    <header className="flex flex-wrap items-center justify-between gap-2 border-b py-2">
      <h2 className="text-sm font-semibold">{t("Request cost comparison")}</h2>
      <Select value={comparison.id} onValueChange={(value) => value && setOtherId(value)}>
        <SelectTrigger aria-label={t("Comparison model")} size="sm" className="max-w-full"><SelectValue /></SelectTrigger>
        <SelectContent><SelectGroup>{options.map((m) => <SelectItem key={m.id} value={m.id}>{modelLabel(m.display_name, m.source_model_id)}</SelectItem>)}</SelectGroup></SelectContent>
      </Select>
    </header>
    <div className="grid min-w-0 lg:grid-cols-[minmax(0,1fr)_minmax(17rem,.48fr)]">
      <div className="grid min-w-0 grid-cols-2 divide-x py-3">
        {[model, comparison].map((item, i) => {
          const costs = prices[i];
          return <div key={i} className="min-w-0 px-3 first:pl-0">
            <p className="text-xs text-muted-foreground">{t(i === 0 ? "Draft base rates" : "Saved base rates")}</p>
            <p className="my-1 break-all font-mono text-xs">{item.source_model_id}</p>
            <output aria-label={t(i === 0 ? "Draft estimate" : "Comparison estimate")} className="block break-all text-xl font-semibold tabular-nums">{costs ? usd.format(costs.reduce((a, b) => a + b, 0)) : "—"}</output>
            <dl className="mt-2 grid gap-1 text-xs">{buckets.map(([, label], j) => <div key={label} className="flex flex-wrap justify-between gap-x-2"><dt className="text-muted-foreground">{t(label)}</dt><dd className="tabular-nums">{costs ? usd.format(costs[j]) : "—"}</dd></div>)}</dl>
          </div>;
        })}
      </div>
      <fieldset className="grid min-w-0 grid-cols-2 gap-2 border-t bg-muted/25 p-3 lg:border-t-0 lg:border-l">
        <legend className="sr-only">{t("Sample tokens")}</legend>
        {buckets.map(([, label], i) => <label key={label} className="grid min-w-0 gap-1 text-xs text-muted-foreground">{t("{bucket} tokens", { bucket: t(label) })}
          <Input type="number" min={0} step={1} value={workload[i]} aria-invalid={!validCount(workload[i])} onChange={(e) => setWorkload(workload.map((n, j) => j === i ? e.target.value : n))} />
        </label>)}
        {!workload.every(validCount) && <p className="col-span-2 text-xs text-destructive">{t("Enter whole, non-negative token counts.")}</p>}
      </fieldset>
    </div>
    <p className="border-t py-2 text-xs text-muted-foreground">{t("Estimated USD at base rates, before context tiers, request multipliers, or time windows.")}</p>
    {models.error && <p role="status" className="pb-2 text-xs text-destructive">{t("Comparison models could not be loaded.")}</p>}
    </div>
  </section>;
}
