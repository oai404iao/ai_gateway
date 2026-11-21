import { useId, useMemo, useState } from "react";
import { Activity, ArrowRight, ChevronLeft, ChevronRight, Layers, Wallet } from "lucide-react";
import { Area, AreaChart, CartesianGrid, ReferenceArea, ReferenceLine, XAxis, YAxis } from "recharts";
import type { CostStatisticsReport, CostStatisticsSummary } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { EmptyState } from "@/components/shared/empty-state";
import { NavigationLink } from "@/components/shared/navigation-link";
import { Button } from "@/components/ui/button";
import { ChartContainer, ChartTooltip, ChartTooltipContent } from "@/components/ui/chart";
import { formatCompactTokens, formatUsd } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { byRequestVolume, modelIdentity, ownLogsLink, usageSlices, type UsageMetric } from "./usage-window";

const metrics = [
  ["request_count", "Requests", Activity],
  ["total_tokens", "Total tokens", Layers],
  ["cost_amount", "Total cost", Wallet],
] as const;

function displayMetric(key: UsageMetric, value: number, locale: string, compact = false) {
  if (key === "cost_amount")
    return compact
      ? new Intl.NumberFormat(locale, {
          style: "currency",
          currency: "USD",
          currencyDisplay: "narrowSymbol",
          maximumFractionDigits: value > 0 && value < 0.01 ? 6 : 2,
        }).format(value)
      : formatUsd(String(value));
  return compact ? formatCompactTokens(value) : value.toLocaleString(locale);
}

export function UsageTrend({
  report,
  comparison,
  comparisonLabel,
  groupByDay = false,
  apiKeyId,
}: {
  report: CostStatisticsReport;
  comparison?: CostStatisticsSummary;
  comparisonLabel?: string;
  groupByDay?: boolean;
  apiKeyId?: string;
}) {
  const { t, locale } = useI18n();
  const [metric, setMetric] = useState<UsageMetric>("request_count");
  const [selected, setSelected] = useState<number | null>(null);
  const uid = useId().replace(/:/g, "");
  const slices = useMemo(() => usageSlices(report, groupByDay), [report, groupByDay]);
  const index = Math.min(selected ?? slices.length - 1, slices.length - 1);
  const slice = slices[index];
  const { series, points, config } = useMemo(() => {
    const ranked = [...report.models].sort(byRequestVolume);
    const top = ranked.slice(0, 3);
    const series = top.map((m, i) => ({
      key: `m${i}`,
      name: top.some((other) => other !== m && other.model === m.model)
        ? `${m.model} · ${apiFormatLabel(m.api_format)}`
        : m.model,
    }));
    if (ranked.length > top.length) series.push({ key: "others", name: t("Other models") });
    const ids = new Map(top.map((m, i) => [modelIdentity(m), `m${i}`]));
    const points = report.buckets.map((b) => {
      const values: Record<string, number> = Object.fromEntries(series.map((s) => [s.key, 0]));
      for (const m of b.models) {
        const key = ids.get(modelIdentity(m)) ?? "others";
        values[key] = (values[key] ?? 0) + Number(m[metric]);
      }
      return { ...values, time: b.started_at };
    });
    const config = Object.fromEntries(
      series.map((s, i) => [s.key, { label: s.name, color: `var(--chart-${i + 1})` }]),
    );
    return { series, points, config };
  }, [report, metric, t]);
  const daily = groupByDay || report.granularity === "day";
  const stamp = (date: string | number, detailed = false) =>
    new Intl.DateTimeFormat(locale, {
      timeZone: "UTC",
      ...(daily || detailed ? { month: "2-digit", day: "2-digit" } : {}),
      ...(!daily || detailed ? { hour: "2-digit", minute: "2-digit", hourCycle: "h23" } : {}),
    }).format(new Date(date));
  const dateTicks = daily ? slices.map((s) => s.first) : points.map((p) => p.time);
  const ticks = dateTicks.filter((_, i) => i % Math.ceil(dateTicks.length / 7) === 0);
  const unit = metric === "cost_amount" ? "USD" : t(metric === "total_tokens" ? "Tokens" : "requests");
  const unpriced = report.summary.request_count > 0 && !report.summary.priced_request_count;

  return (
    <section
      aria-label={t("Usage trends")}
      className="grid min-w-0 gap-5 border-y py-5 md:grid-cols-[minmax(10rem,16%)_minmax(0,1fr)] md:gap-7 md:py-7"
    >
      <div
        role="group"
        aria-label={t("Chart metric")}
        className="grid min-w-0 grid-cols-3 gap-1 border-b pb-3 md:flex md:flex-col md:justify-center md:border-r md:border-b-0 md:pr-4 md:pb-0"
      >
        {metrics.map(([key, label, Icon]) => {
          const value = Number(report.summary[key]);
          const previous = Number(comparison?.[key]);
          const hasDelta = previous > 0 && !(key === "cost_amount" && unpriced);
          const delta = hasDelta ? value / previous - 1 : null;
          return (
            <button
              type="button"
              key={key}
              aria-pressed={metric === key}
              onClick={() => setMetric(key)}
              className={cn(
                "relative flex min-w-0 flex-col items-start gap-1 rounded-md px-2 py-2 text-start outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring md:p-3",
                metric === key &&
                  "bg-accent after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:rounded-full after:bg-chart-1 md:after:inset-y-3 md:after:left-0 md:after:w-0.5 md:after:h-auto",
              )}
            >
              <span className="flex items-center gap-1 text-xs text-muted-foreground">
                <Icon className="size-3.5 shrink-0" />
                {t(label)}
              </span>
              <strong
                className="max-w-full break-all text-xl leading-tight font-semibold tracking-tight tabular-nums md:text-3xl"
                title={displayMetric(key, value, locale)}
              >
                {key === "cost_amount" && unpriced ? "—" : displayMetric(key, value, locale, true)}
              </strong>
              {comparisonLabel && (
                <span className="text-[11px] text-muted-foreground">
                  {delta === null
                    ? "—"
                    : delta.toLocaleString(locale, {
                        style: "percent",
                        maximumFractionDigits: 1,
                        signDisplay: "exceptZero",
                      })}{" "}
                  {comparisonLabel}
                </span>
              )}
            </button>
          );
        })}
      </div>
      <div className="min-w-0">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <div className="flex items-baseline gap-2">
            <h2 className="font-semibold">{t("Model trends")}</h2>
            <span className="text-xs text-muted-foreground">
              {unit} / {t(report.granularity === "hour" ? "hour" : "day")}
            </span>
          </div>
          <div className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
            {series.map((s) => (
              <span className="inline-flex min-w-0 items-center gap-1" key={s.key}>
                <i className="size-2 shrink-0 rounded-full" style={{ background: config[s.key].color }} />
                <span className="max-w-44 truncate" title={s.name}>
                  {s.name}
                </span>
              </span>
            ))}
          </div>
        </div>
        {!report.summary.request_count || !series.length || (unpriced && metric === "cost_amount") ? (
          <EmptyState
            title={t(unpriced && metric === "cost_amount" ? "No priced requests" : "No statistics")}
            description={t(
              unpriced && metric === "cost_amount"
                ? "No request costs were recorded for this filter."
                : "No requests matched the selected filter.",
            )}
            className="min-h-52"
          />
        ) : (
          <ChartContainer config={config} className="aspect-auto h-56 w-full sm:h-64">
            <AreaChart
              data={points}
              accessibilityLayer
              margin={{ left: 2, right: 8, top: 12, bottom: 2 }}
              onClick={(e) => {
                if (e.activeIndex == null) return;
                const time = points[Number(e.activeIndex)]?.time;
                if (time) {
                  const i = slices.findIndex((s) => time >= s.first && time <= s.last);
                  if (i !== -1) setSelected(i);
                }
              }}
            >
              <defs>
                {series.map((s) => (
                  <linearGradient key={s.key} id={`${uid}-${s.key}`} x1="0" y1="0" x2="0" y2="1">
                    <stop stopColor={config[s.key].color} stopOpacity={0.26} />
                    <stop offset="1" stopColor={config[s.key].color} stopOpacity={0.02} />
                  </linearGradient>
                ))}
              </defs>
              <CartesianGrid vertical={false} strokeDasharray="3 4" />
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
                width={42}
                allowDecimals={metric === "cost_amount"}
                tickFormatter={(v) =>
                  Number(v).toLocaleString(locale, { notation: "compact", maximumSignificantDigits: 3 })
                }
                tickLine={false}
                axisLine={false}
              />
              {slice &&
                (slice.first === slice.last ? (
                  <ReferenceLine x={slice.first} stroke="var(--chart-1)" strokeDasharray="3 3" />
                ) : (
                  <ReferenceArea
                    x1={slice.first}
                    x2={slice.last}
                    fill="var(--chart-1)"
                    fillOpacity={0.08}
                  />
                ))}
              <ChartTooltip
                content={
                  <ChartTooltipContent
                    labelFormatter={(v) => (typeof v === "string" ? `${stamp(v, true)} UTC` : v)}
                  />
                }
              />
              {series.map((s) => (
                <Area
                  type="monotone"
                  dataKey={s.key}
                  key={s.key}
                  stroke={config[s.key].color}
                  strokeWidth={2}
                  fill={`url(#${uid}-${s.key})`}
                  isAnimationActive={false}
                />
              ))}
            </AreaChart>
          </ChartContainer>
        )}
        {slice && (
          <div className="mt-2 flex flex-wrap items-center justify-between gap-2 text-xs">
            <div className="flex flex-wrap items-center gap-1.5">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("Previous interval")}
                disabled={index === 0}
                onClick={() => setSelected(index - 1)}
              >
                <ChevronLeft />
              </Button>
              <span aria-live="polite">
                <span className="text-muted-foreground">{t("Selected interval")} </span>
                {stamp(slice.start, !daily)} ·{" "}
                <strong>
                  {metric === "cost_amount" && unpriced
                    ? "—"
                    : displayMetric(metric, slice.values[metric], locale)}
                  {metric !== "cost_amount" && ` ${unit}`}
                </strong>
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("Next interval")}
                disabled={index === slices.length - 1}
                onClick={() => setSelected(index + 1)}
              >
                <ChevronRight />
              </Button>
            </div>
            <NavigationLink
              variant="ghost"
              size="xs"
              to={ownLogsLink(
                {
                  started_after: new Date(slice.start).toISOString(),
                  started_before: new Date(slice.end).toISOString(),
                },
                apiKeyId ? { api_key_id: apiKeyId } : {},
              )}
            >
              {t("Interval logs")}
              <ArrowRight data-icon="inline-end" />
            </NavigationLink>
          </div>
        )}
      </div>
    </section>
  );
}
