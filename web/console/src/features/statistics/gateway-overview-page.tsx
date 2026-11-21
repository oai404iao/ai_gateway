import { useMemo, useState } from "react";
import { ArrowUpRight } from "lucide-react";
import { Link } from "react-router";
import type { CostStatisticsReport } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { NavigationLink } from "@/components/shared/navigation-link";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useOwnRequestLogs } from "@/features/request-logs/api";
import { formatCompactTokens, formatDurationMs, formatUsd } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";
import { useCostStatistics } from "./api";
import { UsageTrend } from "./usage-trend";
import {
  byRequestVolume,
  modelIdentity,
  overviewWindow,
  ownLogsLink,
  requestLogBounds,
  type UsageBounds,
  type UsagePeriod,
} from "./usage-window";

function ModelLedger({ report, bounds }: { report: CostStatisticsReport; bounds: UsageBounds }) {
  const { t, locale } = useI18n();
  const rows = useMemo(() => [...report.models].sort(byRequestVolume), [report.models]);
  if (!rows.length)
    return (
      <p className="py-6 text-sm text-muted-foreground">{t("No requests matched the selected filter.")}</p>
    );
  return (
    <Table className="tabular-nums">
      <TableHeader>
        <TableRow>
          {["Requested model", "Request share", "Requests", "Success rate", "Total tokens", "Total cost"].map((h) => (
            <TableHead key={h}>{t(h)}</TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.slice(0, 10).map((row, i) => {
          const share = report.summary.request_count ? row.request_count / report.summary.request_count : 0;
          const color = `var(--chart-${Math.min(i + 1, 4)})`;
          return (
            <TableRow key={modelIdentity(row)}>
              <TableCell>
                <Link
                  className="inline-flex max-w-64 items-center gap-2 font-medium underline-offset-4 hover:underline"
                  title={t("Related requests for {model}", { model: row.model })}
                  aria-label={t("Related requests for {model}", { model: `${row.model} · ${apiFormatLabel(row.api_format)}` })}
                  to={ownLogsLink(bounds, { model: row.model, api_format: row.api_format })}
                >
                  <i className="size-2 shrink-0 rounded-full" style={{ background: color }} />
                  <span className="truncate">
                    {row.model}
                  </span>
                  <ArrowUpRight aria-hidden className="size-3 shrink-0 text-muted-foreground" />
                </Link>
                <span className="block pl-4 text-xs text-muted-foreground">
                  {apiFormatLabel(row.api_format)}
                </span>
              </TableCell>
              <TableCell>
                <span className="inline-flex items-center gap-2">
                  <i aria-hidden className="hidden h-1 w-16 overflow-hidden rounded-full bg-muted sm:block">
                    <span
                      className="block h-full"
                      style={{ width: `${share * 100}%`, background: color }}
                    />
                  </i>
                  {share.toLocaleString(locale, { style: "percent", maximumFractionDigits: 1 })}
                </span>
              </TableCell>
              <TableCell>{row.request_count.toLocaleString(locale)}</TableCell>
              <TableCell>
                {row.success_rate === null
                  ? "—"
                  : row.success_rate.toLocaleString(locale, { style: "percent", maximumFractionDigits: 1 })}
              </TableCell>
              <TableCell title={row.total_tokens.toLocaleString(locale)}>
                {formatCompactTokens(row.total_tokens)}
              </TableCell>
              <TableCell>{formatUsd(row.cost_amount)}</TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function RequestLedger({ bounds, failed }: { bounds: UsageBounds; failed: boolean }) {
  const { t, locale } = useI18n();
  const query = useOwnRequestLogs({
    ...requestLogBounds(bounds),
    limit: 6,
    ...(failed ? { outcome: "failed" } : {}),
  });
  const clock = new Intl.DateTimeFormat(locale, {
    timeZone: "UTC",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  });
  return (
    <AsyncResource
      isLoading={query.isLoading}
      error={query.error}
      isEmpty={query.data?.length === 0}
      emptyTitle="No request logs"
      emptyDescription="No requests matched the selected filter."
    >
      <Table className="tabular-nums">
        <TableHeader>
          <TableRow>
            {["Time", "Requested model", "Outcome", "Duration", "Total tokens", "Total cost"].map((h) => (
              <TableHead key={h}>
                {t(h)}
                {h === "Time" && " · UTC"}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {query.data?.map((row) => (
            <TableRow key={row.id}>
              <TableCell>
                <Link
                  className="inline-flex items-center gap-1 font-medium underline-offset-4 hover:underline"
                  to={ownLogsLink(bounds, { id: row.id, ...(failed ? { outcome: "failed" } : {}) })}
                >
                  {clock.format(new Date(row.started_at))}
                  <ArrowUpRight className="size-3" aria-hidden />
                </Link>
              </TableCell>
              <TableCell>{row.client_model}</TableCell>
              <TableCell>
                <StatusBadge value={row.outcome} />
              </TableCell>
              <TableCell>{formatDurationMs(row.total_duration_ms)}</TableCell>
              <TableCell>
                {row.input_tokens === null || row.output_tokens === null
                  ? "—"
                  : formatCompactTokens(row.input_tokens + row.output_tokens)}
              </TableCell>
              <TableCell>{formatUsd(row.cost_amount)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </AsyncResource>
  );
}

export function GatewayOverviewPage() {
  const { t, locale } = useI18n();
  const [period, setPeriod] = useState<UsagePeriod>("7d");
  const [tab, setTab] = useState("models");
  const [end] = useState(() => Math.floor(Date.now() / 1000) * 1000);
  const bounds = overviewWindow(end, period);
  const report = useCostStatistics("own", { ...bounds, granularity: "hour" });
  const prior = useCostStatistics("own", { ...overviewWindow(end, period, true), granularity: "day" });
  const range = new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    ...(period === "24h" ? { hour: "2-digit", minute: "2-digit", hourCycle: "h23" } : {}),
    timeZone: "UTC",
  }).formatRange(new Date(bounds.started_after), new Date(end));
  const reportUrl = `/statistics?${new URLSearchParams({ ...bounds, granularity: "hour" })}`;
  return (
    <div className="flex min-w-0 flex-col gap-5">
      <PageHeader
        title="Gateway overview"
        actions={
          <>
            <span className="self-center text-xs text-muted-foreground">{t("Personal usage")} · {range} UTC</span>
            <ToggleGroup
              variant="outline"
              size="sm"
              spacing={0}
              value={[period]}
              onValueChange={(v) => {
                if (v[0] === "7d" || v[0] === "24h") setPeriod(v[0]);
              }}
              aria-label={t("Quick range")}
            >
              <ToggleGroupItem value="24h">{t("Last 24 hours")}</ToggleGroupItem>
              <ToggleGroupItem value="7d">{t("Last 7 days")}</ToggleGroupItem>
            </ToggleGroup>
            <NavigationLink to={reportUrl}>
              {t("Usage report")}
              <ArrowUpRight data-icon="inline-end" />
            </NavigationLink>
          </>
        }
      />
      <AsyncResource isLoading={report.isLoading} error={report.error}>
        {report.data && (
          <UsageTrend
            key={period}
            report={report.data}
            comparison={prior.data?.summary}
            comparisonLabel={t(period === "7d" ? "vs. last week" : "vs. previous day")}
            groupByDay={period === "7d"}
          />
        )}
      </AsyncResource>
      <Tabs value={tab} onValueChange={(v) => setTab(String(v))}>
        <div className="flex flex-wrap items-center justify-between gap-2 border-b pb-1">
          <TabsList variant="line" aria-label={t("Usage details")}>
            <TabsTrigger value="models">{t("Model distribution")}</TabsTrigger>
            <TabsTrigger value="recent">{t("Recent requests")}</TabsTrigger>
            <TabsTrigger value="failed">{t("Failed requests")}</TabsTrigger>
          </TabsList>
          <NavigationLink
            variant="ghost"
            size="xs"
            to={ownLogsLink(bounds, tab === "failed" ? { outcome: "failed" } : {})}
          >
            {t("Request Logs")}
            <ArrowUpRight data-icon="inline-end" />
          </NavigationLink>
        </div>
        <TabsContent value="models">
          {report.data && (
            <>
              <ModelLedger report={report.data} bounds={bounds} />
              {report.data.models.length > 10 && (
                <NavigationLink variant="link" to={reportUrl}>
                  {t("View all models")}
                  <ArrowUpRight data-icon="inline-end" />
                </NavigationLink>
              )}
            </>
          )}
        </TabsContent>
        <TabsContent value="recent">
          {tab === "recent" && <RequestLedger bounds={bounds} failed={false} />}
        </TabsContent>
        <TabsContent value="failed">
          {tab === "failed" && <RequestLedger bounds={bounds} failed />}
        </TabsContent>
      </Tabs>
    </div>
  );
}
