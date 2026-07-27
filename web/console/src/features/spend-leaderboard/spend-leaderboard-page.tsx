import { useMemo, useState } from "react";
import {
  Award,
  ChevronLeft,
  ChevronRight,
  Crown,
  Medal,
  Sparkles,
  Trophy,
} from "lucide-react";
import { Bar, BarChart, Cell, LabelList, XAxis, YAxis } from "recharts";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Progress } from "@/components/ui/progress";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/app/i18n";
import type {
  SpendLeaderboardEntry,
  SpendLeaderboardPeriod,
} from "@/api/types";
import { formatDateTime } from "@/lib/dates";
import { formatCompactTokens, formatUsd } from "@/lib/formatters";
import { cn } from "@/lib/utils";
import { useSpendLeaderboard } from "./api";

interface PodiumDatum {
  rank: number;
  rank_label: string;
  display_name: string;
  cost: number;
  fill: string;
  entry: SpendLeaderboardEntry | null;
}

const LEADERBOARD_LIMIT = 50;
const SHANGHAI_TIME_ZONE = "Asia/Shanghai";

function numericAmount(value: string): number {
  const amount = Number(value);
  return Number.isFinite(amount) ? amount : 0;
}

function initials(value: string): string {
  const result = value
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0])
    .join("")
    .toUpperCase();
  return result || "U";
}

function formatShare(costAmount: string, totalAmount: string): string {
  const total = numericAmount(totalAmount);
  if (total <= 0) return "—";
  return (numericAmount(costAmount) / total).toLocaleString(undefined, {
    style: "percent",
    maximumFractionDigits: 1,
  });
}

function sharePercent(costAmount: string, totalAmount: string): number {
  const total = numericAmount(totalAmount);
  if (total <= 0) return 0;
  return Math.min(100, (numericAmount(costAmount) / total) * 100);
}

function rankVariant(rank: number) {
  if (rank === 1) return "warning" as const;
  if (rank === 2) return "secondary" as const;
  if (rank === 3) return "info" as const;
  return "outline" as const;
}

function rankAccent(rank: number): string {
  if (rank === 1) return "border-warning/40 bg-warning/10";
  if (rank === 2) return "border-border bg-muted/60";
  return "border-info/30 bg-info/10";
}

function rankAvatarAccent(rank: number): string {
  if (rank === 1) return "ring-warning/40";
  if (rank === 2) return "ring-muted-foreground/25";
  return "ring-info/35";
}

function rankFill(rank: number): string {
  if (rank === 1) return "var(--color-gold)";
  if (rank === 2) return "var(--color-silver)";
  return "var(--color-bronze)";
}

function shanghaiDate(value: string): Date {
  return new Date(`${value}T00:00:00+08:00`);
}

function formatPeriod(
  period: SpendLeaderboardPeriod,
  periodStart: string,
  periodEnd: string,
  locale: string,
): string {
  const dateFormatter = new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeZone: SHANGHAI_TIME_ZONE,
  });
  if (period === "day") {
    return dateFormatter.format(shanghaiDate(periodStart));
  }
  if (period === "month") {
    return new Intl.DateTimeFormat(locale, {
      month: "long",
      year: "numeric",
      timeZone: SHANGHAI_TIME_ZONE,
    }).format(shanghaiDate(periodStart));
  }
  const inclusiveEnd = shanghaiDate(periodEnd);
  inclusiveEnd.setUTCDate(inclusiveEnd.getUTCDate() - 1);
  return `${dateFormatter.format(shanghaiDate(periodStart))} – ${dateFormatter.format(inclusiveEnd)}`;
}

function PodiumUser({
  datum,
  rankLabel,
}: {
  datum: PodiumDatum;
  rankLabel: string;
}) {
  if (!datum.entry) {
    return <div aria-hidden="true" className="min-h-28" />;
  }
  const Icon = datum.rank === 1 ? Crown : datum.rank === 2 ? Medal : Award;

  return (
    <div
      className={cn(
        "flex min-w-0 flex-col items-center gap-2 rounded-xl border p-2 text-center",
        rankAccent(datum.rank),
        datum.rank === 1 && "sm:-translate-y-2",
      )}
    >
      <Avatar
        size="lg"
        className={cn(
          "ring-4 ring-background shadow-lg",
          rankAvatarAccent(datum.rank),
        )}
      >
        <AvatarFallback>{initials(datum.entry.display_name)}</AvatarFallback>
      </Avatar>
      <div className="flex min-w-0 flex-col items-center gap-1">
        <Badge variant={rankVariant(datum.rank)}>
          <Icon data-icon="inline-start" />
          <span className="sr-only">{rankLabel} </span>#{datum.rank}
        </Badge>
        <span className="max-w-full truncate font-medium">
          {datum.entry.display_name}
        </span>
        <span className="text-xs text-muted-foreground tabular-nums">
          {formatUsd(datum.entry.cost_amount)}
        </span>
      </div>
    </div>
  );
}

export function SpendLeaderboardPage() {
  const [period, setPeriod] = useState<SpendLeaderboardPeriod>("day");
  const [periodStart, setPeriodStart] = useState<string | undefined>();
  const { t, locale } = useI18n();
  const { data, error, isLoading } = useSpendLeaderboard({
    period,
    period_start: periodStart,
    limit: LEADERBOARD_LIMIT,
  });

  const podiumData = useMemo<PodiumDatum[]>(() => {
    const topEntries = data?.entries.slice(0, 3) ?? [];
    const entriesByRank = new Map(
      topEntries.map((entry) => [entry.rank, entry]),
    );
    return [2, 1, 3].map((rank) => {
      const entry = entriesByRank.get(rank) ?? null;
      return {
        rank,
        rank_label: entry ? `#${rank}` : "",
        display_name: entry?.display_name ?? "",
        cost: entry ? numericAmount(entry.cost_amount) : 0,
        fill: rankFill(rank),
        entry,
      };
    });
  }, [data]);

  const chartConfig = {
    cost: {
      label: t("Cost"),
      color: "var(--warning)",
    },
    gold: {
      label: "#1",
      color: "var(--warning)",
    },
    silver: {
      label: "#2",
      color: "var(--muted-foreground)",
    },
    bronze: {
      label: "#3",
      color: "var(--info)",
    },
  } satisfies ChartConfig;

  const columns: Column<SpendLeaderboardEntry>[] = [
    {
      key: "rank",
      header: t("Rank"),
      className: "w-20",
      render: (entry) => (
        <Badge
          variant={rankVariant(entry.rank)}
          aria-label={t("Rank {rank}", { rank: entry.rank })}
        >
          #{entry.rank}
        </Badge>
      ),
    },
    {
      key: "user",
      header: t("User"),
      render: (entry) => (
        <div className="flex min-w-48 items-center gap-3">
          <Avatar size="sm">
            <AvatarFallback>{initials(entry.display_name)}</AvatarFallback>
          </Avatar>
          <span className="truncate font-medium">{entry.display_name}</span>
        </div>
      ),
    },
    {
      key: "cost",
      header: t("Cost"),
      render: (entry) => (
        <span className="font-medium tabular-nums">
          {formatUsd(entry.cost_amount)}
        </span>
      ),
    },
    {
      key: "share",
      header: t("Share"),
      className: "min-w-32",
      render: (entry) => (
        <div className="flex min-w-28 flex-col gap-1">
          <span className="text-xs font-medium tabular-nums">
            {formatShare(entry.cost_amount, data?.total_cost_amount ?? "0")}
          </span>
          <Progress
            value={sharePercent(
              entry.cost_amount,
              data?.total_cost_amount ?? "0",
            )}
            aria-label={t("Share of spend")}
          />
        </div>
      ),
    },
    {
      key: "priced_requests",
      header: t("Priced requests"),
      render: (entry) => entry.priced_request_count.toLocaleString(),
    },
    {
      key: "tokens",
      header: t("Total tokens"),
      render: (entry) => formatCompactTokens(entry.total_tokens),
    },
  ];

  const selectedPeriodLabel = data
    ? formatPeriod(data.period, data.period_start, data.period_end, locale)
    : t("Loading…");

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Spend leaderboard"
        description="Periodically refreshed user-spend rankings."
      />

      <Card>
        <CardHeader>
          <CardTitle>{t("Ranking period")}</CardTitle>
          <CardDescription>
            {t(
              "Daily rankings run from 00:00 to the following 00:00; weekly rankings run Monday through Sunday; monthly rankings run from the 1st through the final day. All periods use Asia/Shanghai and refresh every 15 minutes.",
            )}
          </CardDescription>
          <CardAction>
            <Badge variant="secondary">{t("Asia/Shanghai")}</Badge>
          </CardAction>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <ToggleGroup
            value={[period]}
            onValueChange={(values) => {
              const value = values[0] as SpendLeaderboardPeriod | undefined;
              if (!value) return;
              setPeriod(value);
              setPeriodStart(undefined);
            }}
            variant="outline"
            spacing={0}
            aria-label={t("Ranking period")}
          >
            {(
              [
                ["day", "Daily"],
                ["week", "Weekly"],
                ["month", "Monthly"],
              ] as const
            ).map(([value, label]) => (
              <ToggleGroupItem
                key={value}
                value={value}
                aria-label={t(label)}
              >
                {t(label)}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant={periodStart ? "outline" : "secondary"}>
              {periodStart ? t("Historical period") : t("Current period")}
            </Badge>
            <span className="text-sm font-medium">{selectedPeriodLabel}</span>
            {data?.refreshed_at ? (
              <span className="text-sm text-muted-foreground">
                {t("Snapshot refreshed {time}", {
                  time: formatDateTime(data.refreshed_at),
                })}
              </span>
            ) : null}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{t("History")}</CardTitle>
          <CardDescription>
            {t("Browse earlier and later retained rankings for this period.")}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            disabled={!data?.previous_period_start}
            onClick={() => setPeriodStart(data?.previous_period_start ?? undefined)}
          >
            <ChevronLeft data-icon="inline-start" />
            {t("Previous period")}
          </Button>
          <Button
            variant={periodStart ? "outline" : "secondary"}
            disabled={!periodStart}
            onClick={() => setPeriodStart(undefined)}
          >
            {t("Current period")}
          </Button>
          <Button
            variant="outline"
            disabled={!data?.next_period_start}
            onClick={() => setPeriodStart(data?.next_period_start ?? undefined)}
          >
            {t("Next period")}
            <ChevronRight data-icon="inline-end" />
          </Button>
        </CardContent>
      </Card>

      <AsyncResource isLoading={isLoading} error={error}>
        {data ? (
          data.entries.length === 0 ? (
            <Card>
              <CardContent>
                <EmptyState
                  title={t("No spend data")}
                  description={t(
                    "No recorded request costs were found for this period.",
                  )}
                  className="py-12"
                />
              </CardContent>
            </Card>
          ) : (
            <>
              <Card>
                <CardHeader>
                  <CardTitle>{t("Top spenders")}</CardTitle>
                  <CardDescription>
                    {t(
                      "The top three users take the podium; the table lists up to {count} users.",
                      { count: LEADERBOARD_LIMIT },
                    )}
                  </CardDescription>
                  <CardAction>
                    <Badge variant="warning">
                      <Sparkles data-icon="inline-start" />
                      {t("Top 3")}
                    </Badge>
                  </CardAction>
                </CardHeader>
                <CardContent className="flex flex-col gap-6">
                  <div className="grid grid-cols-3 gap-3">
                    {podiumData.map((datum) => (
                      <PodiumUser
                        key={datum.rank}
                        datum={datum}
                        rankLabel={t("Rank")}
                      />
                    ))}
                  </div>
                  <div className="rounded-xl border bg-muted/30 px-2 py-4">
                    <ChartContainer config={chartConfig} className="h-72 w-full">
                      <BarChart
                        accessibilityLayer
                        data={podiumData}
                        margin={{ top: 28, right: 12, left: 12, bottom: 0 }}
                      >
                        <XAxis dataKey="display_name" hide />
                        <YAxis hide />
                        <ChartTooltip
                          cursor={false}
                          content={
                            <ChartTooltipContent
                              labelKey="display_name"
                              formatter={(value) => formatUsd(String(value))}
                            />
                          }
                        />
                        <Bar
                          dataKey="cost"
                          radius={[16, 16, 0, 0]}
                          maxBarSize={128}
                        >
                          <LabelList
                            dataKey="rank_label"
                            position="top"
                            className="fill-foreground text-xs font-semibold"
                          />
                          {podiumData.map((datum) => (
                            <Cell key={datum.rank} fill={datum.fill} />
                          ))}
                        </Bar>
                      </BarChart>
                    </ChartContainer>
                  </div>
                  <div className="grid gap-3 sm:grid-cols-2">
                    <div className="flex flex-col gap-1 rounded-lg border bg-muted/30 p-3">
                      <span className="text-xs text-muted-foreground">
                        {t("Total recorded cost")}
                      </span>
                      <span className="text-xl font-medium tabular-nums">
                        {formatUsd(data.total_cost_amount)}
                      </span>
                    </div>
                    <div className="flex flex-col gap-1 rounded-lg border bg-muted/30 p-3">
                      <span className="text-xs text-muted-foreground">
                        {t("Users shown")}
                      </span>
                      <span className="text-xl font-medium tabular-nums">
                        {data.entries.length.toLocaleString()}
                      </span>
                    </div>
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>{t("Leaderboard")}</CardTitle>
                  <CardDescription>
                    {t("Top {count} ranked users for the selected period.", {
                      count: LEADERBOARD_LIMIT,
                    })}
                  </CardDescription>
                  <CardAction>
                    <Trophy className="size-4 text-muted-foreground" />
                  </CardAction>
                </CardHeader>
                <CardContent>
                  <ResourceTable
                    columns={columns}
                    rows={data.entries}
                    rowKey={(entry) => entry.user_id}
                  />
                </CardContent>
              </Card>
            </>
          )
        ) : null}
      </AsyncResource>
    </div>
  );
}
