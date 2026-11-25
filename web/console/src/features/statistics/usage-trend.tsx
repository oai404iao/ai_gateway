import { lazy, Suspense, useId, useMemo, useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import { Activity, ArrowRight, ChevronLeft, ChevronRight, Coins, Layers, Minus, TrendingDown, TrendingUp } from "lucide-react";
import type { CostStatisticsReport, CostStatisticsSummary } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { EmptyState } from "@/components/shared/empty-state";
import { NavigationLink } from "@/components/shared/navigation-link";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Frame, FramePanel } from "@/components/ui/frame";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { formatTokens, formatUsd } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";
import { byRequestVolume, modelIdentity, ownLogsLink, usageSlices, type UsageMetric } from "./usage-window";
import styles from "./usage-trend.module.css";
import type { TrendSeries } from "./usage-trend-plot";

const UsageTrendPlot = lazy(() => import("./usage-trend-plot"));

const metrics = [
  ["request_count", "Requests", Activity],
  ["total_tokens", "Total tokens", Layers],
  ["cost_amount", "Total cost", Coins],
] as const;

function displayMetric(key: UsageMetric, value: number | string, locale: string, compactRequests = false) {
  if (key === "cost_amount") return formatUsd(value);
  if (key === "total_tokens") return formatTokens(Number(value));
  return Number(value).toLocaleString(locale, compactRequests ? { notation: "compact", maximumFractionDigits: 2 } : undefined);
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
  const indicatorId = useId();
  const reducedMotion = useReducedMotion();
  const slices = useMemo(() => usageSlices(report, groupByDay), [report, groupByDay]);
  const index = Math.min(selected ?? slices.length - 1, slices.length - 1);
  const slice = slices[index];
  const series = useMemo(() => {
    const ranked = [...report.models].sort(byRequestVolume);
    const top = ranked.slice(0, 3);
    const series: TrendSeries[] = top.map((m, i) => ({
      key: `m${i}`,
      modelId: modelIdentity(m),
      color: `var(--chart-${i + 1})`,
      name: top.some((other) => other !== m && other.model === m.model)
        ? `${m.model} · ${apiFormatLabel(m.api_format)}`
        : m.model,
    }));
    if (ranked.length > top.length) series.push({ key: "others", name: t("Other models"), color: `var(--chart-${top.length + 1})` });
    return series;
  }, [report.models, t]);
  const daily = groupByDay || report.granularity === "day";
  const stamp = (date: string | number, detailed = false) =>
    new Intl.DateTimeFormat(locale, {
      timeZone: "UTC",
      ...(daily || detailed ? { month: "2-digit", day: "2-digit" } : {}),
      ...(!daily || detailed ? { hour: "2-digit", minute: "2-digit", hourCycle: "h23" } : {}),
    }).format(new Date(date));
  const unit = metric === "cost_amount" ? "USD" : t(metric === "total_tokens" ? "Tokens" : "requests");
  const unpriced = report.summary.request_count > 0 && !report.summary.priced_request_count;

  return (
    <Frame role="region" aria-label={t("Usage trends")} className="min-w-0 gap-1">
      <ToggleGroup
        value={[metric]}
        onValueChange={([next]) => {
          if (metrics.some(([key]) => key === next)) setMetric(next as UsageMetric);
        }}
        aria-label={t("Chart metric")}
        className="grid w-full min-w-0 grid-cols-3 items-stretch gap-1"
      >
        {metrics.map(([key, label, Icon]) => {
          const value = report.summary[key];
          const previous = Number(comparison?.[key]);
          const missing = key === "cost_amount" && unpriced;
          const delta = previous > 0 && !missing ? Number(value) / previous - 1 : null;
          const Trend = delta === null || delta === 0 ? Minus : delta > 0 ? TrendingUp : TrendingDown;
          return (
            <ToggleGroupItem value={key} key={key} className={styles.metric}>
              <span className={styles.label}><Icon aria-hidden="true" />{t(label)}</span>
              <strong className={styles.reading} title={missing ? "—" : displayMetric(key, value, locale)}>
                {missing ? "—" : displayMetric(key, value, locale, true)}
              </strong>
              {comparisonLabel && (
                <span className={styles.comparison}>
                  {delta === null ? "—" : (
                    <Badge variant="secondary" className={styles.delta}>
                      <Trend aria-hidden="true" />
                      {delta.toLocaleString(locale, { style: "percent", maximumFractionDigits: 1, signDisplay: "exceptZero" })}
                    </Badge>
                  )}
                  <span>{comparisonLabel}</span>
                </span>
              )}
              {metric === key && <motion.span
                aria-hidden="true"
                initial={false}
                layoutId={reducedMotion ? undefined : indicatorId}
                className={styles.indicator}
                transition={{ duration: 0.16, ease: "easeOut" }}
              />}
            </ToggleGroupItem>
          );
        })}
      </ToggleGroup>
      <FramePanel className="min-w-0 bg-card p-0">
        <div className="flex flex-wrap items-center justify-between gap-x-5 gap-y-3 px-3 pt-4 pb-2 sm:px-5 sm:pt-5">
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <h2 className="text-base font-semibold tracking-tight">{t("Model trends")}</h2>
            <span className="text-xs text-muted-foreground tabular-nums">
              {unit} / {t(report.granularity === "hour" ? "hour" : "day")} · UTC
            </span>
          </div>
          <div className="flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
            {series.map((s) => (
              <span className="inline-flex min-w-0 items-center gap-1" key={s.key}>
                <i aria-hidden className="h-0.5 w-3 shrink-0 rounded-full" style={{ background: s.color }} />
                <span className="max-w-44 truncate" title={s.name}>
                  {s.name}
                </span>
              </span>
            ))}
          </div>
        </div>
        {!report.summary.request_count || !series.length || (unpriced && metric === "cost_amount") ? (
          <EmptyState
            title={t(unpriced && metric === "cost_amount" ? "No priced requests" : "No requests match these filters")}
            className="min-h-52"
          />
        ) : (
          <Suspense fallback={<Skeleton className="h-44 w-full sm:h-56" aria-label={t("Loading…")} role="status" />}>
            <UsageTrendPlot
              buckets={report.buckets} series={series} metric={metric}
              slices={slices} daily={daily} index={index} onSelect={setSelected}
              stamp={stamp} formatValue={(value) => displayMetric(metric, value, locale)}
            />
          </Suspense>
        )}
        {slice && (
          <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5 border-t px-3 py-2.5 text-xs sm:px-5">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <Button
                variant="outline"
                size="icon-xs"
                aria-label={t("Previous interval")}
                disabled={index === 0}
                onClick={() => setSelected(index - 1)}
              >
                <ChevronLeft />
              </Button>
              <Button
                variant="outline"
                size="icon-xs"
                aria-label={t("Next interval")}
                disabled={index === slices.length - 1}
                onClick={() => setSelected(index + 1)}
              >
                <ChevronRight />
              </Button>
              <span aria-live="polite" className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 tabular-nums">
                <span className="text-muted-foreground">{t("Selected interval")}</span>
                <time dateTime={new Date(slice.start).toISOString()}>{stamp(slice.start, !daily)} UTC</time>
                <strong className="border-l pl-2 font-semibold">
                  {metric === "cost_amount" && unpriced
                    ? "—"
                    : displayMetric(metric, slice.values[metric], locale)}
                  {metric !== "cost_amount" && ` ${unit}`}
                </strong>
              </span>
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
      </FramePanel>
    </Frame>
  );
}
