import { useMemo } from "react";
import { CartesianGrid, Line, LineChart, ReferenceArea, ReferenceLine, XAxis, YAxis } from "recharts";
import type { CostStatisticsBucket } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { ChartContainer, ChartTooltip, ChartTooltipContent } from "@/components/ui/chart";
import { formatTokens, formatUsdAmount } from "@/lib/formatters";
import { modelIdentity, type UsageMetric, type usageSlices } from "./usage-window";

export interface TrendSeries {
  key: string;
  modelId?: string;
  name: string;
  color: string;
}

export default function UsageTrendPlot({
  buckets, series, metric, slices, daily, index, onSelect, stamp, formatValue,
}: {
  buckets: CostStatisticsBucket[];
  series: TrendSeries[];
  metric: UsageMetric;
  slices: ReturnType<typeof usageSlices>;
  daily: boolean;
  index: number;
  onSelect: (index: number) => void;
  stamp: (date: string | number, detailed?: boolean) => string;
  formatValue: (value: number | string) => string;
}) {
  const { locale } = useI18n();
  const points = useMemo(() => {
    const ids = new Map(series.map((s) => [s.modelId, s.key]));
    return buckets.map((b) => {
      const values: Record<string, number> = Object.fromEntries(series.map((s) => [s.key, 0]));
      for (const m of b.models) {
        const key = ids.get(modelIdentity(m)) ?? "others";
        values[key] = (values[key] ?? 0) + Number(m[metric]);
      }
      return { ...values, time: b.started_at };
    });
  }, [buckets, series, metric]);
  const config = Object.fromEntries(series.map((s) => [s.key, { label: s.name, color: s.color }]));
  const slice = slices[index];
  const dateTicks = daily ? slices.map((s) => s.first) : points.map((p) => p.time);
  const ticks = dateTicks.filter((_, i) => i % Math.ceil(dateTicks.length / 7) === 0);

  return (
    <ChartContainer config={config} className="aspect-auto h-44 w-full px-2 text-[11px] tabular-nums sm:h-56 sm:px-4">
      <LineChart
        data={points}
        accessibilityLayer
        margin={{ left: 2, right: 8, top: 12, bottom: 2 }}
        onClick={(e) => {
          if (e.activeIndex == null) return;
          const time = points[Number(e.activeIndex)]?.time;
          if (time) {
            const i = slices.findIndex((s) => time >= s.first && time <= s.last);
            if (i !== -1) onSelect(i);
          }
        }}
      >
        <CartesianGrid vertical={false} stroke="var(--border)" strokeDasharray="2 4" />
        <XAxis
          dataKey="time"
          ticks={ticks}
          tickFormatter={(v) => stamp(v)}
          tickLine={false}
          axisLine={false}
          tickMargin={7}
          minTickGap={12}
        />
        <YAxis
          width={metric === "cost_amount" ? "auto" : 42}
          allowDecimals={metric === "cost_amount"}
          tickFormatter={(v) => metric === "cost_amount" ? formatUsdAmount(v) :
            metric === "total_tokens" ? formatTokens(v) :
            Number(v).toLocaleString(locale, { notation: "compact", maximumSignificantDigits: 3 })}
          tickLine={false}
          axisLine={false}
        />
        {slice && (slice.first === slice.last ? (
          <ReferenceLine x={slice.first} stroke="var(--muted-foreground)" strokeDasharray="3 3" />
        ) : (
          <ReferenceArea x1={slice.first} x2={slice.last} fill="var(--muted-foreground)" fillOpacity={0.07} />
        ))}
        <ChartTooltip content={
          <ChartTooltipContent
            valueFormatter={formatValue}
            labelFormatter={(v) => (typeof v === "string" ? `${stamp(v, true)} UTC` : v)}
          />
        } />
        {series.map((s) => (
          <Line
            type="linear"
            dataKey={s.key}
            key={s.key}
            stroke={s.color}
            strokeWidth={2}
            dot={points.length === 1 ? { r: 3 } : false}
            activeDot={{ r: 4, stroke: "var(--card)", strokeWidth: 2 }}
            isAnimationActive={false}
          />
        ))}
      </LineChart>
    </ChartContainer>
  );
}
