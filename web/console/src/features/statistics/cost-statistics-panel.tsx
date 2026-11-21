import { useMemo, useState } from "react";
import { resourceLabels } from "@/lib/resource-labels";
import {
  Coins,
  Gauge,
  Hash,
  Sparkles,
  Zap,
} from "lucide-react";
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { Link, useSearchParams } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
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
import {
  useAdminApiKeys,
  useRoutingGroups,
  useLogicalChannels,
  useChannelCapabilities,
  useUpstreamAccesses,
  useUsers,
} from "@/features/admin/api";
import { useOwnApiKeys } from "@/features/api-keys/api";
import {
  useCostStatistics,
  type CostStatisticsFilters,
  type CostStatisticsScope,
} from "@/features/statistics/api";
import type {
  CostStatisticsChannel,
  CostStatisticsModel,
  StatisticsGranularity,
} from "@/api/types";
import {
  dateTimeLocalToIso,
  formatDateTime,
  formatDateTimeLocalInput,
} from "@/lib/dates";
import { formatCompactTokens, formatUsd } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";
import { UsageTrend } from "./usage-trend";
import { modelIdentity, overviewWindow, ownLogsLink, type UsageMetric } from "./usage-window";

interface CostFilterDraft {
  started_after: string;
  started_before: string;
  granularity: StatisticsGranularity;
  user_id: string;
  api_key_id: string;
  channel_id: string;
  codex_credential_id: string;
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
    started_after: formatDateTimeLocalInput(startedAt.toISOString(), true),
    started_before: formatDateTimeLocalInput(endedAt.toISOString(), true),
    granularity: "hour",
    user_id: "",
    api_key_id: "",
    channel_id: "",
    codex_credential_id: "",
  };
}

function draftFromSearchParams(
  search: URLSearchParams,
  scope: CostStatisticsScope,
): CostFilterDraft {
  const draft = defaultDraft();
  if (scope === "own" && !search.has("started_after") && !search.has("started_before")) {
    const bounds = overviewWindow(Math.floor(Date.now() / 1000) * 1000, "7d");
    draft.started_after = formatDateTimeLocalInput(bounds.started_after, true);
    draft.started_before = formatDateTimeLocalInput(bounds.started_before, true);
  }
  const startedAfter = search.get("started_after");
  const startedBefore = search.get("started_before");
  const granularity = search.get("granularity");
  if (startedAfter && formatDateTimeLocalInput(startedAfter, true)) {
    draft.started_after = formatDateTimeLocalInput(startedAfter, true);
  }
  if (startedBefore && formatDateTimeLocalInput(startedBefore, true)) {
    draft.started_before = formatDateTimeLocalInput(startedBefore, true);
  }
  if (granularity === "hour" || granularity === "day") {
    draft.granularity = granularity;
  }
  draft.api_key_id = search.get("api_key_id") ?? "";
  if (scope === "system") {
    draft.user_id = search.get("user_id") ?? "";
    draft.channel_id = search.get("channel_id") ?? "";
    draft.codex_credential_id = search.get("codex_credential_id") ?? "";
    if (draft.codex_credential_id) draft.channel_id = "";
  }
  return draft;
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
    started_after: formatDateTimeLocalInput(startedAt.toISOString(), true),
    started_before: formatDateTimeLocalInput(endedAt.toISOString(), true),
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
    channel_id: draft.channel_id || undefined,
    codex_credential_id: draft.codex_credential_id || undefined,
  };
}

function sameCostStatisticsFilters(
  left: CostStatisticsFilters,
  right: CostStatisticsFilters,
): boolean {
  return (
    left.started_after === right.started_after &&
    left.started_before === right.started_before &&
    left.granularity === right.granularity &&
    left.user_id === right.user_id &&
    left.api_key_id === right.api_key_id &&
    left.channel_id === right.channel_id &&
    left.codex_credential_id === right.codex_credential_id
  );
}

function searchParamsForFilters(
  filters: CostStatisticsFilters,
  scope: CostStatisticsScope,
): URLSearchParams {
  const search = new URLSearchParams({
    started_after: filters.started_after,
    started_before: filters.started_before,
    granularity: filters.granularity,
  });
  if (filters.api_key_id) search.set("api_key_id", filters.api_key_id);
  if (scope === "system") {
    if (filters.user_id) search.set("user_id", filters.user_id);
    if (filters.channel_id) search.set("channel_id", filters.channel_id);
    if (filters.codex_credential_id) {
      search.set("codex_credential_id", filters.codex_credential_id);
    }
  }
  return search;
}

function numericAmount(value: string): number {
  const amount = Number(value);
  return Number.isFinite(amount) ? amount : 0;
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

function cacheRate(inputTokens: number, cachedInputTokens: number): number | null {
  return inputTokens > 0 ? cachedInputTokens / inputTokens : null;
}

function SummaryCard({
  title,
  value,
  description,
  icon: Icon,
}: {
  title: string;
  value: React.ReactNode;
  description?: React.ReactNode;
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
        {description && <CardDescription className="text-xs">{description}</CardDescription>}
      </CardContent>
    </Card>
  );
}

export function CostStatisticsPanel({
  scope,
}: {
  scope: CostStatisticsScope;
}) {
  const [searchParams, setSearchParams] = useSearchParams();
  const [modelSort, setModelSort] = useState<UsageMetric>("cost_amount");
  const [initialDraft] = useState(() =>
    draftFromSearchParams(searchParams, scope),
  );
  const [draft, setDraft] = useState<CostFilterDraft>(initialDraft);
  const [filters, setFilters] = useState<CostStatisticsFilters>(
    () => toFilters(initialDraft) as CostStatisticsFilters,
  );
  const [quickRange, setQuickRange] = useState<QuickRange | null>(() =>
    scope === "own" || searchParams.has("started_after") || searchParams.has("started_before")
      ? null
      : "today",
  );
  const { data, isLoading, isFetching, error, refetch } = useCostStatistics(
    scope,
    filters,
  );
  const isSystemView = scope === "system";
  const users = useUsers(isSystemView);
  const adminApiKeys = useAdminApiKeys(isSystemView);
  const channels = useLogicalChannels(isSystemView);
  const channelGroups = useRoutingGroups(isSystemView);
  const capabilities = useChannelCapabilities(isSystemView);
  const accesses = useUpstreamAccesses(isSystemView);
  const ownApiKeys = useOwnApiKeys(!isSystemView);
  const { t } = useI18n();

  const runQuery = (next: CostStatisticsFilters) => {
    setSearchParams(searchParamsForFilters(next, scope), { replace: true });
    if (sameCostStatisticsFilters(filters, next)) {
      void refetch();
      return;
    }
    setFilters(next);
  };

  const userNames = resourceLabels((users.data ?? []).map(user => ({id: user.id, name: user.email ? `${user.display_name} · ${user.email}` : user.display_name})));
  const filteredKeys = isSystemView
    ? (adminApiKeys.data ?? [])
        .filter((key) => !draft.user_id || key.user_id === draft.user_id)
        .map((key) => {
          const ownerName = !draft.user_id && userNames.get(key.user_id);
          return { id: key.id, name: ownerName ? `${key.name} · ${ownerName}` : key.name };
        })
    : (ownApiKeys.data ?? []).map((key) => ({ id: key.id, name: key.name }));
  const keyNames = resourceLabels(filteredKeys);
  const channelGroupNames = new Map(
    (channelGroups.data ?? []).map((group) => [group.id, group.name]),
  );
  const channelOptions = (capabilities.data ?? [])
    .flatMap((capability) => {
      const channel = channels.data?.find((channel) => channel.id === capability.channel_id);
      return channel ? [{
        id: capability.id,
        name: `${channel.name} · ${capability.settings.operation}`,
        groupName: channelGroupNames.get(channel.group_id) ?? t("Unavailable channel group"),
      }] : [];
    })
    .sort(
      (left, right) =>
        left.groupName.localeCompare(right.groupName) ||
        left.name.localeCompare(right.name),
    );
  const codexAccessNames = new Map((accesses.data ?? [])
    .filter(access => access.connector_kind === "codex").map(access => [access.id, access.name]));
  const credentialChannels = new Map<string, Set<string>>();
  for (const channel of channels.data ?? []) {
    const access = codexAccessNames.get(channel.access_id);
    if (!channel.credential_id || !access) continue;
    const labels = credentialChannels.get(channel.credential_id) ?? new Set<string>();
    labels.add(`${access} · ${channel.name}`);
    credentialChannels.set(channel.credential_id, labels);
  }
  const codexCredentialOptions = [...credentialChannels].map(([id, labels]) => ({ id, name: [...labels].sort().join(", ") }))
    .sort((left, right) => left.name.localeCompare(right.name));
  const credentialLabels = resourceLabels(codexCredentialOptions);

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
    runQuery(next);
  };

  const clear = () => {
    const next = defaultDraft();
    setDraft(next);
    runQuery(toFilters(next) as CostStatisticsFilters);
    setQuickRange("today");
  };

  const applyQuickRange = (range: QuickRange) => {
    const next = quickRangeDraft(range, draft);
    setDraft(next);
    runQuery(toFilters(next) as CostStatisticsFilters);
    setQuickRange(range);
  };

  const chart = useMemo(() => {
    if (!data || !isSystemView) {
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
  }, [data, isSystemView, t]);

  const modelRows = useMemo(() => {
    if (!data) return [];
    return [...data.models].sort((left, right) => {
      return Number(right[modelSort]) - Number(left[modelSort]) || right.request_count - left.request_count;
    });
  }, [data, modelSort]);
  const channelRows = useMemo(() => {
    if (!data) return [];
    return [...data.channels].sort((left, right) => {
      const costDelta =
        numericAmount(right.cost_amount) - numericAmount(left.cost_amount);
      return costDelta || right.request_count - left.request_count;
    });
  }, [data]);

  const columns: Column<CostStatisticsModel>[] = [
    {
      key: "model",
      header: t("Model"),
      render: (model) => isSystemView ? <span className="font-medium">{model.model}</span> : (
        <Link className="font-medium underline-offset-4 hover:underline" to={ownLogsLink(filters, {
          model: model.model, api_format: model.api_format,
          ...(filters.api_key_id ? { api_key_id: filters.api_key_id } : {}),
        })} title={t("Related requests for {model}", { model: model.model })}
          aria-label={t("Related requests for {model}", { model: `${model.model} · ${apiFormatLabel(model.api_format)}` })}>
          {model.model}
        </Link>
      ),
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
      render: (model) => formatCompactTokens(model.total_tokens),
    },
    {
      key: "input_tokens",
      header: t("Input tokens"),
      render: (model) => formatCompactTokens(model.input_tokens),
    },
    {
      key: "cached_input_tokens",
      header: t("Cache hit tokens"),
      render: (model) => formatCompactTokens(model.cached_input_tokens),
    },
    {
      key: "cache_rate",
      header: t("Cache rate"),
      render: (model) =>
        formatRate(cacheRate(model.input_tokens, model.cached_input_tokens)),
    },
    {
      key: "cache_write_tokens",
      header: t("Cache write tokens"),
      render: (model) => formatCompactTokens(model.cache_write_tokens),
    },
    {
      key: "output_tokens",
      header: t("Output tokens"),
      render: (model) => formatCompactTokens(model.output_tokens),
    },
    {
      key: "cost",
      header: t("Cost"),
      render: (model) => formatUsd(model.cost_amount),
    },
  ];
  const channelColumns: Column<CostStatisticsChannel>[] = [
    {
      key: "group",
      header: t("Channel group"),
      render: (channel) => (
        <span className="font-medium">{channel.channel_group_name}</span>
      ),
    },
    {
      key: "channel",
      header: t("Channel"),
      render: (channel) => <span className="font-medium">{channel.name}</span>,
    },
    {
      key: "format",
      header: t("Format"),
      render: (channel) => (
        <StatusBadge
          value={channel.api_format}
          label={apiFormatLabel(channel.api_format)}
          variant="info"
        />
      ),
    },
    {
      key: "requests",
      header: t("Requests"),
      render: (channel) => channel.request_count.toLocaleString(),
    },
    {
      key: "success",
      header: t("Success rate"),
      render: (channel) => formatRate(channel.success_rate),
    },
    {
      key: "tokens",
      header: t("Total tokens"),
      render: (channel) => formatCompactTokens(channel.total_tokens),
    },
    {
      key: "input_tokens",
      header: t("Input tokens"),
      render: (channel) => formatCompactTokens(channel.input_tokens),
    },
    {
      key: "cached_input_tokens",
      header: t("Cache hit tokens"),
      render: (channel) => formatCompactTokens(channel.cached_input_tokens),
    },
    {
      key: "cache_rate",
      header: t("Cache rate"),
      render: (channel) =>
        formatRate(cacheRate(channel.input_tokens, channel.cached_input_tokens)),
    },
    {
      key: "cache_write_tokens",
      header: t("Cache write tokens"),
      render: (channel) => formatCompactTokens(channel.cache_write_tokens),
    },
    {
      key: "output_tokens",
      header: t("Output tokens"),
      render: (channel) => formatCompactTokens(channel.output_tokens),
    },
    {
      key: "cost",
      header: t("Cost"),
      render: (channel) => formatUsd(channel.cost_amount),
    },
  ];

  const filterFields = (
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
                step={1}
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
                step={1}
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
            {isSystemView ? (
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
                      {draft.user_id && !userNames.has(draft.user_id) && <SelectItem value={draft.user_id} disabled>{t("Unavailable")}</SelectItem>}
                      {users.data?.map((user) => (
                        <SelectItem key={user.id} value={user.id}>
                          {userNames.get(user.id)}
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
                    {draft.api_key_id && !filteredKeys.some(key => key.id === draft.api_key_id) && <SelectItem value={draft.api_key_id} disabled>{t("Unavailable")}</SelectItem>}
                    {filteredKeys.map((key) => (
                      <SelectItem key={key.id} value={key.id}>
                        {keyNames.get(key.id)}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            {isSystemView ? (
              <Field>
                <FieldLabel htmlFor="statistics_codex_credential">
                  {t("Codex credential")}
                </FieldLabel>
                <Select
                  value={draft.codex_credential_id || "__all__"}
                  onValueChange={(value) =>
                    setDraft((current) => ({
                      ...current,
                      codex_credential_id:
                        value === "__all__" ? "" : value,
                      channel_id: value === "__all__" ? current.channel_id : "",
                    }))
                  }
                >
                  <SelectTrigger id="statistics_codex_credential">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">
                        {t("All Codex credentials")}
                      </SelectItem>
                      {codexCredentialOptions.map((credential) => (
                        <SelectItem key={credential.id} value={credential.id}>
                          {credentialLabels.get(credential.id)}
                        </SelectItem>
                      ))}
                      {draft.codex_credential_id && !credentialChannels.has(draft.codex_credential_id) && <SelectItem value={draft.codex_credential_id} disabled>{t("Unavailable")}</SelectItem>}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            {isSystemView ? (
              <Field>
                <FieldLabel htmlFor="statistics_channel">{t("Channel")}</FieldLabel>
                <Select
                  value={draft.channel_id || "__all__"}
                  onValueChange={(value) =>
                    setDraft((current) => ({
                      ...current,
                      channel_id: value === "__all__" ? "" : value,
                      codex_credential_id:
                        value === "__all__"
                          ? current.codex_credential_id
                          : "",
                    }))
                  }
                >
                  <SelectTrigger id="statistics_channel">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                    <SelectItem value="__all__">{t("All channels")}</SelectItem>
                    {draft.channel_id && !channelOptions.some(channel => channel.id === draft.channel_id) && <SelectItem value={draft.channel_id} disabled>{t("Unavailable")}</SelectItem>}
                      {channelOptions.map((channel) => (
                        <SelectItem key={channel.id} value={channel.id}>
                          {channel.groupName} · {channel.name}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            <Field className="justify-end">
              <FieldLabel className="sr-only">{t("Filter actions")}</FieldLabel>
              <div className="flex gap-2">
                <Button onClick={apply} disabled={isFetching}>
                  {isFetching ? <Spinner data-icon="inline-start" /> : null}
                  {t("Apply")}
                </Button>
                <Button variant="outline" onClick={clear}>
                  {t("Clear")}
                </Button>
              </div>
            </Field>
          </FieldGroup>
  );

  return (
    <div className="flex min-w-0 flex-col gap-6">
      {isSystemView ? <Card>
        <CardHeader><CardTitle>{t("Filters")}</CardTitle></CardHeader>
        <CardContent>{filterFields}</CardContent>
      </Card> : <details className="rounded-md border">
        <summary className="cursor-pointer px-4 py-3 text-sm font-medium focus-visible:outline-ring">{t("Filters")}<span className="ml-3 text-xs font-normal text-muted-foreground">{formatDateTime(filters.started_after)} – {formatDateTime(filters.started_before)} · {t(filters.granularity === "hour" ? "Hourly" : "Daily")}</span></summary>
        <div className="px-4 pb-4">{filterFields}</div>
      </details>}

      <AsyncResource isLoading={isLoading} error={error}>
        {data ? (
          <>
            {isSystemView ? <><div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
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
                icon={Coins}
              />
              <SummaryCard
                title={t("Total tokens")}
                value={formatCompactTokens(data.summary.total_tokens)}
                description={
                  <div className="grid grid-cols-2 gap-x-3 gap-y-1">
                    <span>
                      {t("Input tokens")}:{" "}
                      {formatCompactTokens(data.summary.input_tokens)}
                    </span>
                    <span>
                      {t("Cache hit tokens")}:{" "}
                      {formatCompactTokens(data.summary.cached_input_tokens)}
                    </span>
                    <span>
                      {t("Cache write tokens")}:{" "}
                      {formatCompactTokens(data.summary.cache_write_tokens)}
                    </span>
                    <span>
                      {t("Output tokens")}:{" "}
                      {formatCompactTokens(data.summary.output_tokens)}
                    </span>
                    <span className="col-span-2">
                      {t("Cache rate")}:{" "}
                      {formatRate(
                        cacheRate(
                          data.summary.input_tokens,
                          data.summary.cached_input_tokens,
                        ),
                      )}
                    </span>
                  </div>
                }
                icon={Sparkles}
              />
              <SummaryCard
                title={t("Average RPM")}
                value={data.summary.average_rpm.toLocaleString(undefined, {
                  maximumFractionDigits: 2,
                })}
                icon={Gauge}
              />
              <SummaryCard
                title={t("Average TPM")}
                value={formatCompactTokens(data.summary.average_tpm)}
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

            </> : <>
              <UsageTrend key={`${data.started_at}/${data.ended_at}/${data.granularity}/${filters.api_key_id ?? ""}`} report={data} groupByDay={data.granularity === "hour" && data.buckets.length > 48} apiKeyId={filters.api_key_id}/>
              <dl className="flex flex-wrap gap-x-6 gap-y-3 text-xs tabular-nums">
                {[
                  ["Input tokens", formatCompactTokens(data.summary.input_tokens)],
                  ["Cache hit tokens", formatCompactTokens(data.summary.cached_input_tokens)],
                  ["Cache write tokens", formatCompactTokens(data.summary.cache_write_tokens)],
                  ["Cache rate", formatRate(cacheRate(data.summary.input_tokens, data.summary.cached_input_tokens))],
                  ["Output tokens", formatCompactTokens(data.summary.output_tokens)],
                  ["Priced requests", data.summary.priced_request_count.toLocaleString()],
                  ["Average RPM", data.summary.average_rpm.toLocaleString(undefined, { maximumFractionDigits: 2 })],
                  ["Average TPM", formatCompactTokens(data.summary.average_tpm)],
                ].map(([label,value]) => <div key={label}><dt className="text-muted-foreground">{t(label)}</dt><dd className="mt-1 font-medium">{value}</dd></div>)}
              </dl>
            </>}

            <Card>
              <CardHeader>
                <CardTitle>{t("Model cost breakdown")}</CardTitle>
                {!isSystemView && <CardAction>
                  <Select value={modelSort} onValueChange={value => {
                    if (value === "cost_amount" || value === "request_count" || value === "total_tokens") setModelSort(value);
                  }} items={{cost_amount:t("Total cost"), request_count:t("Requests"), total_tokens:t("Total tokens")}}>
                    <SelectTrigger aria-label={t("Sort models by")}><SelectValue /></SelectTrigger>
                    <SelectContent><SelectGroup>
                      <SelectItem value="cost_amount">{t("Total cost")}</SelectItem>
                      <SelectItem value="request_count">{t("Requests")}</SelectItem>
                      <SelectItem value="total_tokens">{t("Total tokens")}</SelectItem>
                    </SelectGroup></SelectContent>
                  </Select>
                </CardAction>}
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

            {isSystemView ? (
              <Card>
                <CardHeader>
                  <CardTitle>{t("Channel details")}</CardTitle>
                </CardHeader>
                <CardContent>
                  {channelRows.length === 0 ? (
                    <EmptyState
                      title={t("No channel statistics")}
                      description={t(
                        "No routed requests matched the selected filter.",
                      )}
                      className="py-12"
                    />
                  ) : (
                    <ResourceTable
                      columns={channelColumns}
                      rows={channelRows}
                      rowKey={(channel) =>
                        `${channel.id}:${channel.channel_group_id}:${channel.api_format}`
                      }
                    />
                  )}
                </CardContent>
              </Card>
            ) : null}
          </>
        ) : null}
      </AsyncResource>
    </div>
  );
}
