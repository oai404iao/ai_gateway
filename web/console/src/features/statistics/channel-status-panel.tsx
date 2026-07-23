import { Fragment, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChannelStatus } from "@/features/statistics/api";
import type {
  ChannelStatusBucket,
  ChannelStatusChannelModel,
  ChannelStatusModelMetric,
  ChannelStatusReport,
  ChannelStatusWindow,
} from "@/api/types";
import { formatDateTime } from "@/lib/dates";
import { formatDurationMs } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

const WINDOWS: Array<{ value: ChannelStatusWindow; label: string; shortLabel: string }> = [
  { value: "24h", label: "Last 24 hours", shortLabel: "24h" },
  { value: "3d", label: "Last 3 days", shortLabel: "3d" },
  { value: "7d", label: "Last 7 days", shortLabel: "7d" },
];

function formatRate(value: number | null): string {
  if (value === null) return "—";
  return `${(value * 100).toLocaleString(undefined, { maximumFractionDigits: 1 })}%`;
}

function formatTps(value: number | null): string {
  if (value === null) return "—";
  return `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })} tok/s`;
}

function statusClass(bucket: ChannelStatusBucket | undefined): string {
  if (!bucket || bucket.request_count === 0 || bucket.success_rate === null) return "bg-muted";
  if (bucket.success_rate >= 0.98) return "bg-success";
  if (bucket.success_rate >= 0.9) return "bg-warning";
  return "bg-destructive";
}

function statusLabel(bucket: ChannelStatusBucket | undefined, startedAt: Date): string {
  if (!bucket) return `${formatDateTime(startedAt.toISOString())}: no requests`;
  return `${formatDateTime(bucket.started_at)}: ${formatRate(bucket.success_rate)}, ${bucket.request_count} requests`;
}

function StatusHistory({
  report,
  model,
}: {
  report: ChannelStatusReport;
  model: ChannelStatusChannelModel;
}) {
  const buckets = useMemo(() => {
    const bucketMs = report.bucket_seconds * 1000;
    const startedAt = new Date(report.started_at).getTime();
    const endedAt = new Date(report.ended_at).getTime();
    const count = Math.max(1, Math.ceil((endedAt - startedAt) / bucketMs));
    const history = new Map(
      model.history.map((bucket) => [new Date(bucket.started_at).getTime(), bucket]),
    );
    return Array.from({ length: count }, (_, index) => {
      const timestamp = startedAt + index * bucketMs;
      return {
        startedAt: new Date(timestamp),
        bucket: history.get(timestamp),
      };
    });
  }, [model.history, report.bucket_seconds, report.ended_at, report.started_at]);

  return (
    <div
      className="flex flex-col gap-1"
      role="img"
      aria-label={`${model.model} ${model.request_count} requests, ${formatRate(model.success_rate)} success rate`}
    >
      <div
        className="grid gap-0.5"
        style={{ gridTemplateColumns: `repeat(${buckets.length}, minmax(0, 1fr))` }}
      >
        {buckets.map(({ startedAt, bucket }) => (
          <span
            key={startedAt.toISOString()}
            className={cn("h-5 rounded-[2px]", statusClass(bucket))}
            title={statusLabel(bucket, startedAt)}
            aria-hidden="true"
          />
        ))}
      </div>
      <div className="flex justify-between text-xs text-muted-foreground">
        <span>{formatDateTime(report.started_at)}</span>
        <span>{formatDateTime(report.ended_at)}</span>
      </div>
    </div>
  );
}

function MetricSummary({ metric }: { metric: ChannelStatusChannelModel }) {
  const { t } = useI18n();
  return (
    <dl className="flex flex-wrap gap-x-6 gap-y-1 text-xs">
      <div className="flex items-baseline gap-1">
        <dt className="text-muted-foreground">{t("P90 TTFT")}</dt>
        <dd className="font-medium">{formatDurationMs(metric.p90_ttft_ms)}</dd>
      </div>
      <div className="flex items-baseline gap-1">
        <dt className="text-muted-foreground">{t("P50 TPS")}</dt>
        <dd className="font-medium">{formatTps(metric.p50_tps)}</dd>
      </div>
      <div className="flex items-baseline gap-1">
        <dt className="text-muted-foreground">{t("Success rate")}</dt>
        <dd className="font-medium">{formatRate(metric.success_rate)}</dd>
      </div>
      <div className="flex items-baseline gap-1">
        <dt className="text-muted-foreground">{t("Requests")}</dt>
        <dd className="font-medium">{metric.request_count.toLocaleString()}</dd>
      </div>
    </dl>
  );
}

export function ChannelStatusPanel() {
  const [window, setWindow] = useState<ChannelStatusWindow>("24h");
  const { data, isLoading, error } = useChannelStatus(window);
  const { t } = useI18n();

  const columns: Column<ChannelStatusModelMetric>[] = [
    {
      key: "model",
      header: t("Model"),
      render: (metric) => <span className="font-medium">{metric.model}</span>,
    },
    {
      key: "format",
      header: t("Format"),
      render: (metric) => (
        <StatusBadge
          value={metric.api_format}
          label={apiFormatLabel(metric.api_format)}
          variant="info"
        />
      ),
    },
    {
      key: "ttft",
      header: t("P90 TTFT"),
      render: (metric) => formatDurationMs(metric.p90_ttft_ms),
    },
    {
      key: "tps",
      header: t("P50 TPS"),
      render: (metric) => formatTps(metric.p50_tps),
    },
    {
      key: "success",
      header: t("Success rate"),
      render: (metric) => formatRate(metric.success_rate),
    },
    {
      key: "requests",
      header: t("Requests"),
      render: (metric) => metric.request_count.toLocaleString(),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("Model overview")}</CardTitle>
          <CardDescription>
            {t("Metrics aggregated across channels included in status statistics.")}
          </CardDescription>
          <CardAction>
            <ToggleGroup
              value={[window]}
              onValueChange={(values) => {
                const value = values[0];
                if (value) setWindow(value as ChannelStatusWindow);
              }}
              variant="outline"
              size="sm"
              spacing={0}
              aria-label={t("Status window")}
            >
              {WINDOWS.map((item) => (
                <ToggleGroupItem
                  key={item.value}
                  value={item.value}
                  aria-label={t(item.label)}
                >
                  <span className="sm:hidden">{item.shortLabel}</span>
                  <span className="hidden sm:inline">{t(item.label)}</span>
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </CardAction>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={data?.channels.length === 0}
            emptyTitle="No tracked channels"
            emptyDescription="Enable status statistics on at least one channel."
          >
            <ResourceTable
              columns={columns}
              rows={data?.models ?? []}
              rowKey={(metric) => `${metric.api_format}:${metric.model}`}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      {data?.channels.map((channel) => (
        <Card key={channel.id}>
          <CardHeader>
            <CardTitle className="flex flex-wrap items-center gap-2">
              <span>{channel.name}</span>
              <Badge variant="secondary">{channel.channel_group_name}</Badge>
              <StatusBadge
                value={channel.api_format}
                label={apiFormatLabel(channel.api_format)}
                variant="info"
              />
            </CardTitle>
            <CardDescription>
              {channel.enabled ? t("Routing enabled") : t("Routing disabled")}
              {channel.auto_disabled ? ` · ${t("Auto-disabled")}` : ""}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-5">
            {channel.models.length === 0 ? (
              <EmptyState
                title={t("No channel models")}
                description={t("Add available upstream models to this channel.")}
                className="py-8"
              />
            ) : (
              channel.models.map((model, index) => (
                <Fragment key={`${model.api_format}:${model.model}`}>
                  {index > 0 ? <Separator /> : null}
                  <div className="flex flex-col gap-3">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <span className="font-medium">{model.model}</span>
                      <MetricSummary metric={model} />
                    </div>
                    <StatusHistory report={data} model={model} />
                  </div>
                </Fragment>
              ))
            )}
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
