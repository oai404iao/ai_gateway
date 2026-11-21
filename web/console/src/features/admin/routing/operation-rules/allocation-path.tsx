import { GitBranch, TriangleAlert } from "lucide-react";
import type { OperationTierInput } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { selectionStrategyLabel } from "@/lib/permissions";

interface Props {
  tiers: OperationTierInput[];
  active: number;
  onSelect: (index: number) => void;
  channelName: (capabilityId: string) => string;
  errorFor: (path: Array<string | number>) => string | undefined;
  controls: string;
}

export function AllocationPath({ tiers, active, onSelect, channelName, errorFor, controls }: Props) {
  const { t, locale } = useI18n();
  const percent = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 });
  const ordered = tiers.map((tier, index) => ({ tier, index })).sort((a, b) => a.tier.priority - b.tier.priority);
  return <section aria-label={t("Allocation path")} className="min-w-0 py-3 @min-[42rem]:pr-3">
    <h3 className="mb-3 flex items-center gap-2 text-sm font-medium"><GitBranch className="size-4 text-muted-foreground" />{t("Allocation path")}</h3>
    <ol className="ml-2 grid border-l pl-3">
      {ordered.map(({ tier, index }) => {
        const tierPath = ["routing_tiers", index];
        const invalid = errorFor([...tierPath, "priority"]) || errorFor([...tierPath, "candidates"]) || tier.candidates.some((_, row) =>
          ["capability_id", "upstream_model", "weight"].some((key) => errorFor([...tierPath, "candidates", row, key])));
        const total = tier.candidates.every((row) => row.weight > 0) ? tier.candidates.reduce((sum, row) => sum + row.weight, 0) : 0;
        return <li key={index} className="relative min-w-0 before:absolute before:top-5 before:-left-3 before:w-3 before:border-t">
          <button type="button" aria-label={t("Inspect tier {number}", { number: index + 1 })} aria-pressed={index === active}
            aria-controls={controls} onClick={() => onSelect(index)}
            className="grid w-full gap-1.5 rounded-sm border-l-2 border-transparent p-2 text-left hover:bg-muted/60 focus-visible:outline-2 focus-visible:outline-ring aria-pressed:border-primary aria-pressed:bg-accent">
            <span className="flex flex-wrap items-center gap-2 font-medium"><span className="font-mono text-xs">P{tier.priority}</span><span className="text-xs">{selectionStrategyLabel(tier.selection_strategy)}</span>{invalid && <TriangleAlert aria-label={t("Review the highlighted fields.")} className="size-3.5 text-destructive" />}</span>
            {tier.candidates.length === 0 && <span className="text-xs text-muted-foreground">{t("{count} routing candidates", { count: 0 })}</span>}
            {tier.candidates.map((row, i) => <span key={i} className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-2 border-t border-border/60 pt-1 text-xs">
              <span className="break-all">{channelName(row.capability_id)}</span>
              <span className="row-span-2 text-right font-mono tabular-nums">{total ? percent.format(row.weight / total) : "—"}</span>
              <span className="break-all font-mono text-muted-foreground">{row.upstream_model || "—"}</span>
            </span>)}
          </button>
        </li>;
      })}
    </ol>
    {tiers.length > 0 && <p className="mt-2 text-xs text-muted-foreground">{t("Shares assume all candidates in the same tier are eligible.")}</p>}
  </section>;
}
