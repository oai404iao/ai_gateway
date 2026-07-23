import { useMemo, useState } from "react";
import {
  Coins,
  Gauge,
  Hash,
  Sparkles,
  Zap,
} from "lucide-react";
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { toast } from "sonner";
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
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { useAdminApiKeys, useUsers } from "@/features/admin/api";
import { useOwnApiKeys } from "@/features/api-keys/api";
import {
  useCostStatistics,
  type CostStatisticsFilters,
} from "@/features/statistics/api";
import type {
  CostStatisticsModel,
  StatisticsGranularity,
} from "@/api/types";
import {
  dateTimeLocalToIso,
  formatDateTime,
  formatDateTimeLocalInput,
} from "@/lib/dates";
import { formatTokens, formatUsd } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";
import { useSession } from "@/lib/use-session";

interface CostFilterDraft {
  started_after: string;
  started_before: string;
  granularity: StatisticsGranularity;
  user_id: string;
  api_key_id: string;
}

type QuickRange = "today" | "this_week" | "this_month";

const CHART_COLORS = [
  "var(--success)",
  "var(--info)",
  "var(--warning)",
  "var(--primary)",
  "var(--destructive)",
];

function defaultDraft(): CostFilterDraft {
  const endedAt = new Date();
  endedAt.setSeconds(0, 0);
  const startedAt = new Date(endedAt);
  startedAt.setHours(0, 0, 0, 0);
  return {
    started_after: formatDateTimeLocalInput(startedAt.toISOString()),
    started_before: formatDateTimeLocalInput(endedAt.toISOString()),
    granularity: "hour",
    user_id: "",
    api_key_id: "",
  };
}

function quickRangeDraft(
  range: QuickRange,
  current: CostFilterDraft,
  now = new Date(),
): CostFilterDraft {
  const endedAt = new Date(now);
  endedAt.setSeconds(0, 0);
  const startedAt = new Date(endedAt);
  if (range === "today") {
    startedAt.setHours(0, 0, 0, 0);
  } else if (range === "this_week") {
    const daysSinceMonday = (startedAt.getDay() + 6) % 7;
    startedAt.setDate(startedAt.getDate() - daysSinceMonday);
    startedAt.setHours(0, 0, 0, 0);
  } else {
    startedAt.setDate(1);
    startedAt.setHours(0, 0, 0, 0);
  }
  return {
    ...current,
    started_after: formatDateTimeLocalInput(startedAt.toISOString()),
    started_before: formatDateTimeLocalInput(endedAt.toISOString()),
    granularity: range === "today" ? "hour" : "day",
  };
}

function toFilters(draft: CostFilterDraft): CostStatisticsFilters | null {
  const startedAfter = dateTimeLocalToIso(draft.started_after);
  const startedBefore = dateTimeLocalToIso(draft.started_before);
  if (!startedAfter || !startedBefore) return null;
  return {
    started_after: startedAfter,
    started_before: startedBefore,
    granularity: draft.granularity,
    user_id: draft.user_id || undefined,
    api_key_id: draft.api_key_id || undefined,
  };
}

function numericAmount(value: string): number {
  const amount = Number(value);
  return Number.isFinite(amount) ? amount : 0;
}

function modelIdentity(model: { api_format: string; model: string }): string {
  return `${model.api_format}:${model.model}`;
}

function formatRate(value: number | null): string {
  if (value === null) return "—";
  return `${(value * 100).toLocaleString(undefined, { maximumFractionDigits: 1 })}%`;
}

function formatCompact(value: number): string {
  return value.toLocaleString(undefined, {
    notation: "compact",
    maximumFractionDigits: 2,
  });
}

function SummaryCard({
  title,
  value,
  description,
  icon: Icon,
}: {
  title: string;
  value: React.ReactNode;
  description: string;
  icon: typeof Hash;
}) {
  return (
    <Card size="sm">
      <CardHeader>
        <CardDescription>{title}</CardDescription>
        <CardAction>
          <Icon className="size-4 text-muted-foreground" />
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-1">
        <CardTitle className="text-2xl tabular-nums">{value}</CardTitle>
        <CardDescription className="text-xs">{description}</CardDescription>
      </CardContent>
    </Card>
  );
}

export function CostStatisticsPanel() {
  const initialDraft = useMemo(defaultDraft, []);
  const [draft, setDraft] = useState<CostFilterDraft>(initialDraft);
  const [filters, setFilters] = useState<CostStatisticsFilters>(
    () => toFilters(initialDraft) as CostStatisticsFilters,
  );
  const [quickRange, setQuickRange] = useState<QuickRange | null>("today");
  const { data, isLoading, error } = useCostStatistics(filters);
  const { user } = useSession();
  const isAdmin = user?.role === "admin";
  const users = useUsers(isAdmin);
  const adminApiKeys = useAdminApiKeys(isAdmin);
  const ownApiKeys = useOwnApiKeys(!isAdmin);
  const { t } = useI18n();

  const filteredKeys = isAdmin
    ? (adminApiKeys.data ?? [])
        .filter((key) => !draft.user_id || key.user_id === draft.user_id)
        .map((key) => ({ id: key.id, name: key.name }))
    : (ownApiKeys.data ?? []).map((key) => ({ id: key.id, name: key.name }));

  const apply = () => {
    const next = toFilters(draft);
    if (!next) {
      toast.error(t("Enter a valid statistics time range."));
      return;
    }
    const startedAt = new Date(next.started_after).getTime();
    const endedAt = new Date(next.started_before).getTime();
    if (startedAt >= endedAt) {
      toast.error(t("The start time must be before the end time."));
      return;
    }
    const maxDays = next.granularity === "hour" ? 31 : 366;
    if (endedAt - startedAt > maxDays * 24 * 60 * 60 * 1000) {
      toast.error(
        t("The selected {granularity} range cannot exceed {days} days.", {
          granularity: t(next.granularity === "hour" ? "hourly" : "daily"),
          days: maxDays,
        }),
      );
      return;
    }
    setFilters(next);
  };

  const clear = () => {
    const next = defaultDraft();
    setDraft(next);
    setFilters(toFilters(next) as CostStatisticsFilters);
    setQuickRange("today");
  };

  const applyQuickRange = (range: QuickRange) => {
    const next = quickRangeDraft(range, draft);
    setDraft(next);
    setFilters(toFilters(next) as CostStatisticsFilters);
    setQuickRange(range);
  };

  const chart = useMemo(() => {
    if (!data) {
      return { config: {} satisfies ChartConfig, data: [], series: [] };
    }
    const ranked = data.models
      .map((model) => ({ model, amount: numericAmount(model.cost_amount) }))
      .filter((item) => item.amount > 0)
      .sort((left, right) => right.amount - left.amount);
    const topModels = ranked.slice(0, 7);
    const topIdentities = new Set(topModels.map((item) => modelIdentity(item.model)));
    const series: Array<{
      key: string;
      label: string;
      identity: string | null;
    }> = topModels.map((item, index) => ({
      key: `model_${index}`,
      label: item.model.model,
      identity: modelIdentity(item.model),
    }));
    if (ranked.length > topModels.length) {
      series.push({
        key: "other_models",
        label: t("Other models"),
        identity: null,
      });
    }
    const config = Object.fromEntries(
      series.map((item, index) => [
        item.key,
        {
          label: item.label,
          color: CHART_COLORS[index % CHART_COLORS.length],
        },
      ]),
    ) satisfies ChartConfig;
    const points = data.buckets.map((bucket) => {
      const point: Record<string, number | string> = { bucket: bucket.started_at };
      for (const item of series) {
        point[item.key] =
          item.identity === null
            ? bucket.models
                .filter((model) => !topIdentities.has(modelIdentity(model)))
                .reduce((sum, model) => sum + numericAmount(model.cost_amount), 0)
            : numericAmount(
                bucket.models.find((model) => modelIdentity(model) === item.identity)
                  ?.cost_amount ?? "0",
              );
      }
      return point;
    });
    return { config, data: points, series };
  }, [data, t]);

  const modelRows = useMemo(() => {
    if (!data) return [];
    return [...data.models].sort((left, right) => {
      const costDelta =
        numericAmount(right.cost_amount) - numericAmount(left.cost_amount);
      return costDelta || right.request_count - left.request_count;
    });
  }, [data]);

  const columns: Column<CostStatisticsModel>[] = [
    {
      key: "model",
      header: t("Model"),
      render: (model) => <span className="font-medium">{model.model}</span>,
    },
    {
      key: "format",
      header: t("Format"),
      render: (model) => (
        <StatusBadge
          value={model.api_format}
          label={apiFormatLabel(model.api_format)}
          variant="info"
        />
      ),
    },
    {
      key: "requests",
      header: t("Requests"),
      render: (model) => model.request_count.toLocaleString(),
    },
    {
      key: "success",
      header: t("Success rate"),
      render: (model) => formatRate(model.success_rate),
    },
    {
      key: "tokens",
      header: t("Total tokens"),
      render: (model) => formatTokens(model.total_tokens),
    },
    {
      key: "cost",
      header: t("Cost"),
      render: (model) => formatUsd(model.cost_amount),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("Filters")}</CardTitle>
          <CardDescription>
            {t(
              isAdmin
                ? "Filter by time range, user, API key, and aggregation granularity."
                : "Filter your own statistics by time range, API key, and aggregation granularity.",
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <FieldGroup className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            <Field className="md:col-span-2 xl:col-span-3">
              <FieldLabel id="statistics-quick-range-label">
                {t("Quick range")}
              </FieldLabel>
              <ToggleGroup
                value={quickRange ? [quickRange] : []}
                onValueChange={(values) => {
                  const value = values[0];
                  if (value) applyQuickRange(value as QuickRange);
                }}
                variant="outline"
                spacing={0}
                aria-labelledby="statistics-quick-range-label"
              >
                {(
                  [
                    ["today", "Today"],
                    ["this_week", "This week"],
                    ["this_month", "This month"],
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
            </Field>
            <Field>
              <FieldLabel htmlFor="statistics_started_after">{t("From")}</FieldLabel>
              <Input
                id="statistics_started_after"
                type="datetime-local"
                value={draft.started_after}
                onChange={(event) => {
                  setQuickRange(null);
                  setDraft((current) => ({
                    ...current,
                    started_after: event.target.value,
                  }));
                }}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="statistics_started_before">{t("To")}</FieldLabel>
              <Input
                id="statistics_started_before"
                type="datetime-local"
                value={draft.started_before}
                onChange={(event) => {
                  setQuickRange(null);
                  setDraft((current) => ({
                    ...current,
                    started_before: event.target.value,
                  }));
                }}
              />
            </Field>
            <Field>
              <FieldLabel>{t("Granularity")}</FieldLabel>
              <ToggleGroup
                value={[draft.granularity]}
                onValueChange={(values) => {
                  const value = values[0];
                  if (value) {
                    setQuickRange(null);
                    setDraft((current) => ({
                      ...current,
                      granularity: value as StatisticsGranularity,
                    }));
                  }
                }}
                variant="outline"
                spacing={0}
                aria-label={t("Granularity")}
              >
                <ToggleGroupItem value="hour" aria-label={t("Hourly")}>
                  {t("Hourly")}
                </ToggleGroupItem>
                <ToggleGroupItem value="day" aria-label={t("Daily")}>
                  {t("Daily")}
                </ToggleGroupItem>
              </ToggleGroup>
            </Field>
            {isAdmin ? (
              <Field>
                <FieldLabel htmlFor="statistics_user">{t("User")}</FieldLabel>
                <Select
                  value={draft.user_id || "__all__"}
                  onValueChange={(value) => {
                    const userId = value === "__all__" ? "" : value;
                    setDraft((current) => ({
                      ...current,
                      user_id: userId,
                      api_key_id:
                        current.api_key_id &&
                        adminApiKeys.data?.some(
                          (key) =>
                            key.id === current.api_key_id &&
                            (!userId || key.user_id === userId),
                        )
                          ? current.api_key_id
                          : "",
                    }));
                  }}
                >
                  <SelectTrigger id="statistics_user">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">{t("All users")}</SelectItem>
                      {users.data?.map((user) => (
                        <SelectItem key={user.id} value={user.id}>
                          {user.display_name}
                          {user.email ? ` · ${user.email}` : ""}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            <Field>
              <FieldLabel htmlFor="statistics_api_key">{t("API key")}</FieldLabel>
              <Select
                value={draft.api_key_id || "__all__"}
                onValueChange={(value) =>
                  setDraft((current) => ({
                    ...current,
                    api_key_id: value === "__all__" ? "" : value,
                  }))
                }
              >
                <SelectTrigger id="statistics_api_key">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="__all__">{t("All API keys")}</SelectItem>
                    {filteredKeys.map((key) => (
                      <SelectItem key={key.id} value={key.id}>
                        {key.name} · {key.id.slice(0, 8)}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field className="justify-end">
              <FieldLabel className="sr-only">{t("Filter actions")}</FieldLabel>
              <div className="flex gap-2">
                <Button onClick={apply}>{t("Apply")}</Button>
                <Button variant="outline" onClick={clear}>
                  {t("Clear")}
                </Button>
              </div>
            </Field>
          </FieldGroup>
        </CardContent>
      </Card>

      <AsyncResource isLoading={isLoading} error={error}>
        {data ? (
          <>
            <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
              <SummaryCard
                title={t("Requests")}
                value={data.summary.request_count.toLocaleString()}
                description={t("{count} priced requests", {
                  count: data.summary.priced_request_count,
                })}
                icon={Hash}
              />
              <SummaryCard
                title={t("Total cost")}
                value={
                  data.summary.priced_request_count > 0 ? (
                    formatUsd(data.summary.cost_amount)
                  ) : (
                    "—"
                  )
                }
                description={t("All costs are settled in USD.")}
                icon={Coins}
              />
              <SummaryCard
                title={t("Total tokens")}
                value={formatCompact(data.summary.total_tokens)}
                description={formatTokens(data.summary.total_tokens)}
                icon={Sparkles}
              />
              <SummaryCard
                title={t("Average RPM")}
                value={data.summary.average_rpm.toLocaleString(undefined, {
                  maximumFractionDigits: 2,
                })}
                description={t("Requests per minute across the selected range.")}
                icon={Gauge}
              />
              <SummaryCard
                title={t("Average TPM")}
                value={formatCompact(data.summary.average_tpm)}
                description={t("Tokens per minute across the selected range.")}
                icon={Zap}
              />
            </div>

            <Card>
              <CardHeader>
                <CardTitle>{t("Cost trend by model")}</CardTitle>
                <CardDescription>
                  {t("UTC buckets displayed in your browser's local time.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                {chart.series.length === 0 ? (
                  <EmptyState
                    title={t("No priced requests")}
                    description={t("No request costs were recorded for this filter.")}
                    className="py-12"
                  />
                ) : (
                  <ChartContainer config={chart.config} className="h-80 w-full">
                    <BarChart accessibilityLayer data={chart.data}>
                      <CartesianGrid vertical={false} />
                      <XAxis
                        dataKey="bucket"
                        tickLine={false}
                        axisLine={false}
                        tickMargin={8}
                        minTickGap={24}
                        tickFormatter={(value) => formatDateTime(String(value))}
                      />
                      <YAxis
                        tickLine={false}
                        axisLine={false}
                        width={48}
                        tickFormatter={(value) => formatCompact(Number(value))}
                      />
                      <ChartTooltip
                        cursor={false}
                        content={
                          <ChartTooltipContent
                            indicator="line"
                            labelFormatter={(value) => formatDateTime(String(value))}
                          />
                        }
                      />
                      <ChartLegend content={<ChartLegendContent className="flex-wrap" />} />
                      {chart.series.map((item) => (
                        <Bar
                          key={item.key}
                          dataKey={item.key}
                          stackId="cost"
                          fill={`var(--color-${item.key})`}
                          maxBarSize={48}
                        />
                      ))}
                    </BarChart>
                  </ChartContainer>
                )}
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Model cost breakdown")}</CardTitle>
                <CardDescription>
                  {t("Requests, reliability, tokens, and cost for each upstream model.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                {modelRows.length === 0 ? (
                  <EmptyState
                    title={t("No statistics")}
                    description={t("No requests matched the selected filter.")}
                    className="py-12"
                  />
                ) : (
                  <ResourceTable
                    columns={columns}
                    rows={modelRows}
                    rowKey={(model) => `${model.api_format}:${model.model}`}
                  />
                )}
              </CardContent>
            </Card>
          </>
        ) : null}
      </AsyncResource>
    </div>
  );
}
