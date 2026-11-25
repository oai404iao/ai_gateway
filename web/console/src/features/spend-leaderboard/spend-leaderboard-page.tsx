import { useId, useRef, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import type { SpendLeaderboardEntry, SpendLeaderboardPeriod, SpendLeaderboardReport } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { ErrorAlert } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { TablePagination } from "@/components/shared/table-pagination";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { decimalPercentShare } from "@/lib/decimal";
import { formatTokens, formatUsd } from "@/lib/formatters";
import { useSpendLeaderboard } from "./api";

const LEADERBOARD_LIMIT = 50;
const PAGE_SIZES = [10, 20, 50] as const;
const SHANGHAI_TIME_ZONE = "Asia/Shanghai";
const PERIODS = [["day", "Daily ranking"], ["week", "Weekly ranking"], ["month", "Monthly ranking"]] as const;
const HEADINGS = [
  ["Rank", "w-[9%]"],
  ["User", "w-[22%]"],
  ["Recorded spend (USD)", "w-[19%] text-right"],
  ["Share of period total", "w-[18%]"],
  ["Priced requests", "w-[16%] text-right"],
  ["Total tokens", "w-[16%] text-right"],
] as const;

function shanghaiDate(value: string): Date {
  return new Date(`${value}T00:00:00+08:00`);
}

function formatPeriod({ period, period_start, period_end }: SpendLeaderboardReport, locale: string): string {
  const start = shanghaiDate(period_start);
  if (period === "month") {
    return new Intl.DateTimeFormat(locale, {
      month: "long", year: "numeric", timeZone: SHANGHAI_TIME_ZONE,
    }).format(start);
  }
  const dates = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: SHANGHAI_TIME_ZONE });
  if (period === "day") return dates.format(start);
  const inclusiveEnd = shanghaiDate(period_end);
  inclusiveEnd.setUTCDate(inclusiveEnd.getUTCDate() - 1);
  return `${dates.format(start)} – ${dates.format(inclusiveEnd)}`;
}

function Rank({ value }: { value: number }) {
  const { t } = useI18n();
  return (
    <span className="tabular-nums">
      <span className="sr-only">{t("Rank {rank}", { rank: value })}</span>
      {value === 1 ? <Badge aria-hidden="true" className="min-w-6 justify-center">1</Badge> :
        <span aria-hidden="true" className="inline-block min-w-6 text-center">{value}</span>}
    </span>
  );
}

function Share({ value, compact = false }: { value: ReturnType<typeof decimalPercentShare>; compact?: boolean }) {
  const { t, locale } = useI18n();
  const format = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 });
  const share = value === null ? "—" : value.belowTenth
    ? `<${format.format(0.001)}` : format.format(value.roundedRatio);
  return (
    <div className="flex items-center gap-2 text-xs tabular-nums">
      {value !== null && (
        <span aria-hidden="true" className="h-1 min-w-0 flex-1 rounded-full bg-muted">
          <span className="block h-full rounded-full bg-muted-foreground"
            style={{ width: `${Math.min(value.percent, 100)}%` }} />
        </span>
      )}
      <span className={compact ? "ml-auto w-24 shrink-0 text-right" : "ml-auto w-12 shrink-0 text-right"}>
        {compact && <span className="text-muted-foreground">{t("Share")} </span>}{share}
      </span>
    </div>
  );
}

function RankingRows({ entries, total, labelId }: {
  entries: SpendLeaderboardEntry[];
  total: string;
  labelId: string;
}) {
  const { t, locale } = useI18n();
  const rows = entries.map((entry) => ({
    entry,
    amount: formatUsd(entry.cost_amount),
    share: decimalPercentShare(entry.cost_amount, total),
    requests: entry.priced_request_count.toLocaleString(locale),
    tokens: formatTokens(entry.total_tokens),
  }));

  return (
    <>
      <div className="hidden @min-[45rem]:block">
        <Table aria-labelledby={labelId} className="table-fixed">
          <TableHeader>
            <TableRow>
              {HEADINGS.map(([label, width]) => (
                <TableHead key={label} scope="col" className={`h-auto py-2 whitespace-normal ${width}`}>{t(label)}</TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map(({ entry, amount, share, requests, tokens }) => (
              <TableRow key={entry.user_id}>
                <TableCell className="py-3"><Rank value={entry.rank} /></TableCell>
                <TableCell className="font-medium whitespace-normal wrap-anywhere">{entry.display_name}</TableCell>
                <TableCell className="text-right text-base font-semibold whitespace-normal wrap-anywhere tabular-nums">{amount}</TableCell>
                <TableCell><Share value={share} /></TableCell>
                <TableCell className="text-right whitespace-normal wrap-anywhere tabular-nums">{requests}</TableCell>
                <TableCell className="text-right whitespace-normal wrap-anywhere tabular-nums">{tokens}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <ol role="list" aria-labelledby={labelId} className="divide-y @min-[45rem]:hidden">
        {rows.map(({ entry, amount, share, requests, tokens }) => (
          <li key={entry.user_id} className="flex min-w-0 flex-col gap-2 py-3">
            <div className="flex flex-wrap items-baseline justify-between gap-x-2 gap-y-1">
              <div className="flex min-w-0 flex-[1_1_8rem] items-baseline gap-2">
                <Rank value={entry.rank} />
                <span className="min-w-0 font-medium wrap-anywhere">{entry.display_name}</span>
              </div>
              <span className="ml-auto max-w-full text-right text-base font-semibold wrap-anywhere tabular-nums">{amount}</span>
            </div>
            <Share value={share} compact />
            <dl className="flex flex-wrap gap-x-4 gap-y-1 text-xs tabular-nums">
              <div className="flex min-w-0 gap-1.5">
                <dt className="text-muted-foreground">{t("Priced requests")}</dt><dd className="wrap-anywhere">{requests}</dd>
              </div>
              <div className="flex min-w-0 gap-1.5">
                <dt className="text-muted-foreground">{t("Total tokens")}</dt><dd className="wrap-anywhere">{tokens}</dd>
              </div>
            </dl>
          </li>
        ))}
      </ol>
    </>
  );
}

export function SpendLeaderboardPage() {
  const [period, setPeriod] = useState<SpendLeaderboardPeriod>("day");
  const [periodStart, setPeriodStart] = useState<string | undefined>();
  const [requestedPage, setPage] = useState(1);
  const [pageSize, setPageSize] = useState<number>(20);
  const { t, locale } = useI18n();
  const rankingsHeading = useId();
  const periodStatus = useRef<HTMLSpanElement>(null);
  const { data, error, isLoading, isFetching, refetch } = useSpendLeaderboard({
    period, period_start: periodStart, limit: LEADERBOARD_LIMIT,
  });
  const periodLabel = data ? formatPeriod(data, locale) : t(isLoading ? "Loading…" : "Period unavailable");
  const count = data?.entries.length ?? 0;
  const pageCount = Math.max(1, Math.ceil(count / pageSize));
  const page = Math.min(requestedPage, pageCount);
  const hasSnapshot = Boolean(data?.refreshed_at || count);
  const changeStart = (start?: string) => {
    setPeriodStart(start);
    setPage(1);
  };

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageHeader title="Spend leaderboard" />
      <div className="flex min-w-0 flex-wrap items-center gap-x-5 gap-y-2">
        <ToggleGroup value={[period]} variant="outline" spacing={0} aria-label={t("Ranking period")}
          className="max-w-full items-stretch"
          onValueChange={([value]) => {
            const next = PERIODS.find(([type]) => type === value)?.[0];
            if (!next || next === period) return;
            setPeriod(next);
            changeStart();
          }}>
          {PERIODS.map(([value, label]) => (
            <ToggleGroupItem key={value} value={value} className="h-auto min-h-8 min-w-0 shrink py-1 text-center whitespace-normal">
              {t(label)}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2">
          <div className="flex min-w-0 max-w-full items-center gap-2">
            <Button variant="outline" size="icon-sm" aria-label={t("Previous period")} title={t("Previous period")}
              aria-disabled={!data?.previous_period_start} className="aria-disabled:opacity-50"
              onClick={() => { if (data?.previous_period_start) changeStart(data.previous_period_start); }}>
              <ChevronLeft />
            </Button>
            <div className="flex min-w-0 flex-col">
              <span ref={periodStatus} tabIndex={-1} role="status" className="text-sm font-medium wrap-anywhere">{periodLabel}</span>
              <span className="text-xs text-muted-foreground">{t("Shanghai time · Asia/Shanghai")}</span>
            </div>
            <Button variant="outline" size="icon-sm" aria-label={t("Next period")} title={t("Next period")}
              aria-disabled={!data?.next_period_start} className="aria-disabled:opacity-50"
              onClick={() => { if (data?.next_period_start) changeStart(data.next_period_start); }}>
              <ChevronRight />
            </Button>
          </div>
          <Button variant={periodStart ? "outline" : "secondary"} size="sm" aria-disabled={!periodStart}
            onClick={() => { if (periodStart) changeStart(); }}>
            {periodStart ? t("Back to current period") : t("Current ranking period")}
          </Button>
        </div>
      </div>

      {error && (
        <div className="flex flex-col gap-2">
          <ErrorAlert error={error} />
          <Button variant="outline" className="self-start" disabled={isFetching} onClick={() => {
            periodStatus.current?.focus();
            void refetch();
          }}>{t("Retry")}</Button>
        </div>
      )}

      {isLoading && (
        <Card variant="section" role="status" aria-label={t("Loading…")}>
          <CardHeader aria-hidden="true"><Skeleton className="h-16 w-full" /></CardHeader>
          <CardContent aria-hidden="true" className="flex flex-col gap-3">
            {[0, 1, 2].map((row) => <Skeleton key={row} className="h-14 w-full" />)}
          </CardContent>
        </Card>
      )}

      {data && (
        <Card variant="section" className="@container gap-0 pt-0">
          {hasSnapshot && (
            <CardHeader className="flex flex-wrap items-end justify-between gap-x-6 gap-y-2 border-b py-4">
              <div className="flex min-w-0 flex-col gap-1">
                <p className="text-xs text-muted-foreground">{t("Total recorded cost for the period")}</p>
                <p className="text-3xl font-semibold tracking-tight wrap-anywhere tabular-nums sm:text-4xl">{formatUsd(data.total_cost_amount)}</p>
              </div>
              <div className="flex min-w-0 flex-col gap-1 text-xs text-muted-foreground sm:text-right">
                {data.refreshed_at && (
                  <time dateTime={data.refreshed_at}>
                    {t("Snapshot refreshed {time}", { time: new Intl.DateTimeFormat(locale, {
                      dateStyle: "medium", timeStyle: "short", timeZone: SHANGHAI_TIME_ZONE, hourCycle: "h23",
                    }).format(new Date(data.refreshed_at)) })}
                  </time>
                )}
                <p>{t("Generated every 15 minutes")}</p>
              </div>
            </CardHeader>
          )}
          <CardContent className="flex min-w-0 flex-col gap-3 pt-3">
            <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
              <h2 id={rankingsHeading} className="text-base font-medium">{t("User rankings")}</h2>
              {count > 0 && <p className="text-xs text-muted-foreground">
                {t("Showing {count} users, up to {limit}.", { count, limit: LEADERBOARD_LIMIT })}
              </p>}
            </div>
            {count === 0 ? (
              <EmptyState title={t(hasSnapshot ? "No priced requests in this period" : "No snapshot yet")} />
            ) : (
              <RankingRows entries={data.entries.slice((page - 1) * pageSize, page * pageSize)}
                total={data.total_cost_amount} labelId={rankingsHeading} />
            )}
            {count > PAGE_SIZES[0] && (
              <TablePagination page={page} pageSize={pageSize} pageCount={pageCount} totalItems={count}
                pageSizeOptions={PAGE_SIZES} onPageChange={setPage} onPageSizeChange={(size) => {
                  setPageSize(size);
                  setPage(1);
                }} />
            )}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
