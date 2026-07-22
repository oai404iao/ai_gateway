import { Fragment } from "react";
import { RefreshCw } from "lucide-react";
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
import { Progress } from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";
import { AsyncResource } from "@/components/shared/async-resource";
import { useSystemLoad } from "@/features/statistics/api";
import type {
  SystemDatabasePoolLoad,
  SystemQueueLoad,
} from "@/api/types";
import { formatDateTime } from "@/lib/dates";
import { formatBytes } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";

type BadgeVariant =
  | "default"
  | "secondary"
  | "destructive"
  | "success"
  | "warning"
  | "info";

function formatPercent(value: number | null): string {
  if (value === null) return "—";
  return `${value.toLocaleString(undefined, { maximumFractionDigits: 1 })}%`;
}

function formatLoad(value: number | null): string {
  if (value === null) return "—";
  return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function formatAge(seconds: number | null): string {
  if (seconds === null) return "—";
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3_600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  if (seconds < 86_400) {
    const hours = Math.floor(seconds / 3_600);
    return `${hours}h ${Math.floor((seconds % 3_600) / 60)}m`;
  }
  const days = Math.floor(seconds / 86_400);
  return `${days}d ${Math.floor((seconds % 86_400) / 3_600)}h`;
}

function pressureStatus(value: number | null): {
  label: string;
  variant: BadgeVariant;
} {
  if (value === null) return { label: "Unavailable", variant: "secondary" };
  if (value >= 90) return { label: "Critical", variant: "destructive" };
  if (value >= 70) return { label: "Elevated", variant: "warning" };
  return { label: "Normal", variant: "success" };
}

function MetricCard({
  title,
  value,
  description,
  percent,
  status,
}: {
  title: string;
  value: React.ReactNode;
  description: string;
  percent?: number | null;
  status?: { label: string; variant: BadgeVariant };
}) {
  const { t } = useI18n();
  const resolvedStatus = status ?? pressureStatus(percent ?? null);
  return (
    <Card size="sm">
      <CardHeader>
        <CardDescription>{title}</CardDescription>
        <CardAction>
          <Badge variant={resolvedStatus.variant}>{t(resolvedStatus.label)}</Badge>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <CardTitle className="text-2xl tabular-nums">{value}</CardTitle>
        {percent !== undefined && percent !== null ? (
          <Progress value={Math.min(100, Math.max(0, percent))} />
        ) : null}
        <CardDescription className="text-xs">{description}</CardDescription>
      </CardContent>
    </Card>
  );
}

function QueueRow({
  label,
  description,
  queue,
}: {
  label: string;
  description: string;
  queue: SystemQueueLoad;
}) {
  const { t } = useI18n();
  const status = pressureStatus(queue.utilization_percent);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-col gap-1">
          <span className="font-medium">{label}</span>
          <span className="text-xs text-muted-foreground">{description}</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-sm tabular-nums">
            {queue.capacity > 0
              ? `${queue.depth.toLocaleString()} / ${queue.capacity.toLocaleString()}`
              : "—"}
          </span>
          <Badge variant={status.variant}>{t(status.label)}</Badge>
        </div>
      </div>
      {queue.utilization_percent !== null ? (
        <Progress value={queue.utilization_percent} />
      ) : null}
    </div>
  );
}

function BacklogRow({
  label,
  value,
  description,
}: {
  label: string;
  value: React.ReactNode;
  description: string;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3">
      <div className="flex flex-col gap-1">
        <span className="font-medium">{label}</span>
        <span className="text-xs text-muted-foreground">{description}</span>
      </div>
      <span className="text-sm font-medium tabular-nums">{value}</span>
    </div>
  );
}

function PoolRow({
  label,
  pool,
}: {
  label: string;
  pool: SystemDatabasePoolLoad;
}) {
  const { t } = useI18n();
  const status = pressureStatus(pool.utilization_percent);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-col gap-1">
          <span className="font-medium">{label}</span>
          <span className="text-xs text-muted-foreground">
            {t("{used} of {capacity} connections in use", {
              used: pool.in_use,
              capacity: pool.capacity,
            })}
          </span>
        </div>
        <Badge variant={status.variant}>{t(status.label)}</Badge>
      </div>
      {pool.utilization_percent !== null ? (
        <Progress value={pool.utilization_percent} />
      ) : null}
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <span>
          {t("Open")}: {pool.size.toLocaleString()}
        </span>
        <span>
          {t("Idle")}: {pool.idle.toLocaleString()}
        </span>
      </div>
    </div>
  );
}

export function SystemLoadPanel() {
  const query = useSystemLoad();
  const { t } = useI18n();
  const data = query.data;
  const pipelineFailures = data
    ? data.request_log.spool_append_failures_total +
      data.request_log.ingress_failures_total +
      data.request_log.projection_failures_total +
      data.request_log.settlement_failures_total
    : 0;
  const queues = data
    ? [
        {
          label: t("Request-log notifications"),
          description: t("Wakeups for durable spool ingestion."),
          queue: data.queues.request_log_notifications,
        },
        {
          label: t("Projection notifications"),
          description: t("Wakeups for ingress-to-query-table projection."),
          queue: data.queues.request_log_projection,
        },
        {
          label: t("Automatic disable"),
          description: t("Upstream error events waiting for control-plane changes."),
          queue: data.queues.automatic_disable,
        },
      ]
    : [];

  return (
    <div className="flex flex-col gap-6">
      <Card>
        <CardHeader>
          <CardTitle>{t("Current instance")}</CardTitle>
          <CardDescription>
            {t("Auto-refreshes every 5 seconds. Metrics describe this gateway process, not a cluster aggregate.")}
          </CardDescription>
          <CardAction>
            <Button
              variant="outline"
              size="sm"
              disabled={query.isFetching}
              onClick={() => void query.refetch()}
            >
              <RefreshCw data-icon="inline-start" />
              {t("Refresh")}
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent>
          <AsyncResource isLoading={query.isLoading} error={query.error}>
            {data ? (
              <div className="flex flex-wrap gap-x-6 gap-y-2 text-sm text-muted-foreground">
                <span>
                  {t("Last sampled")}: {formatDateTime(data.sampled_at)}
                </span>
                <span>
                  {t("Gateway uptime")}: {formatAge(data.uptime_seconds)}
                </span>
                <span>
                  {t("Started")}: {formatDateTime(data.started_at)}
                </span>
              </div>
            ) : null}
          </AsyncResource>
        </CardContent>
      </Card>

      {data ? (
        <>
            <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
              <MetricCard
                title={t("Host CPU")}
                value={formatPercent(data.host.cpu_usage_percent)}
                description={t("{count} logical CPUs · load {one} / {five} / {fifteen}", {
                  count: data.host.logical_cpu_count,
                  one: formatLoad(data.host.load_average_1m),
                  five: formatLoad(data.host.load_average_5m),
                  fifteen: formatLoad(data.host.load_average_15m),
                })}
                percent={data.host.cpu_usage_percent}
              />
              <MetricCard
                title={t("Host memory")}
                value={formatPercent(data.host.memory_usage_percent)}
                description={`${formatBytes(data.host.memory_used_bytes)} / ${formatBytes(
                  data.host.memory_total_bytes,
                )}`}
                percent={data.host.memory_usage_percent}
              />
              <MetricCard
                title={t("Gateway CPU")}
                value={formatPercent(data.process.cpu_usage_percent)}
                description={t("Share of total host CPU capacity.")}
                percent={data.process.cpu_usage_percent}
              />
              <MetricCard
                title={t("Gateway memory")}
                value={formatBytes(data.process.resident_memory_bytes)}
                description={t("{percent} of host memory (RSS).", {
                  percent: formatPercent(data.process.resident_memory_percent),
                })}
                percent={data.process.resident_memory_percent}
              />
              <MetricCard
                title={t("In-flight requests")}
                value={data.runtime.in_flight_requests.toLocaleString()}
                description={t("{routing} routed upstream · {window} in RPM windows", {
                  routing: data.runtime.routing_in_flight_requests,
                  window: data.runtime.requests_in_current_windows,
                })}
                status={{ label: "Live", variant: "info" }}
              />
            </div>

            <div className="grid gap-6 xl:grid-cols-2">
              <Card>
                <CardHeader>
                  <CardTitle>{t("Bounded queues")}</CardTitle>
                  <CardDescription>
                    {t("Depth and capacity for process-local asynchronous work queues.")}
                  </CardDescription>
                </CardHeader>
                <CardContent className="flex flex-col gap-4">
                  {queues.map((item, index) => (
                    <Fragment key={item.label}>
                      {index > 0 ? <Separator /> : null}
                      <QueueRow {...item} />
                    </Fragment>
                  ))}
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>{t("Request-log durability")}</CardTitle>
                  <CardDescription>
                    {t("Local spool, PostgreSQL ingress, and billing settlement pressure.")}
                  </CardDescription>
                  <CardAction>
                    <Badge variant={pipelineFailures > 0 ? "destructive" : "success"}>
                      {t(pipelineFailures > 0 ? "Failures recorded" : "No failures")}
                    </Badge>
                  </CardAction>
                </CardHeader>
                <CardContent className="flex flex-col gap-4">
                  <BacklogRow
                    label={t("Spool pending")}
                    value={formatBytes(data.request_log.spool_pending_bytes)}
                    description={t("Durable bytes not yet committed to ingress.")}
                  />
                  <Separator />
                  <BacklogRow
                    label={t("Ingress backlog")}
                    value={
                      data.request_log.ingress_backlog_rows_estimate === null
                        ? "—"
                        : data.request_log.ingress_backlog_rows_estimate.toLocaleString()
                    }
                    description={t("Estimated rows · oldest {age}", {
                      age: formatAge(data.request_log.ingress_oldest_age_seconds),
                    })}
                  />
                  <Separator />
                  <BacklogRow
                    label={t("Settlement backlog")}
                    value={
                      data.request_log.settlement_backlog_rows === null
                        ? "—"
                        : data.request_log.settlement_backlog_rows.toLocaleString()
                    }
                    description={t("Billable rows · oldest {age}", {
                      age: formatAge(data.request_log.settlement_oldest_age_seconds),
                    })}
                  />
                  <Separator />
                  <BacklogRow
                    label={t("Pipeline failures")}
                    value={pipelineFailures.toLocaleString()}
                    description={t("Cumulative spool, ingress, projection, and settlement failures.")}
                  />
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>{t("Runtime state")}</CardTitle>
                  <CardDescription>
                    {t("Process-local admission, routing, and operating-system counters.")}
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <dl className="grid gap-4 sm:grid-cols-2">
                    {[
                      [t("Tracked API keys"), data.runtime.tracked_api_keys],
                      [t("Tracked channels"), data.runtime.tracked_channels],
                      [t("Cooling down"), data.runtime.cooling_down_channels],
                      [t("Half-open"), data.runtime.half_open_channels],
                      [t("Affinity entries"), data.runtime.session_affinity_entries],
                      [t("Open file descriptors"), data.process.open_file_descriptors],
                      [t("Threads"), data.process.threads],
                    ].map(([label, value]) => (
                      <div key={String(label)} className="flex items-center justify-between gap-3">
                        <dt className="text-sm text-muted-foreground">{label}</dt>
                        <dd className="font-medium tabular-nums">
                          {typeof value === "number" ? value.toLocaleString() : "—"}
                        </dd>
                      </div>
                    ))}
                  </dl>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>{t("Database pools")}</CardTitle>
                  <CardDescription>
                    {t("Current checked-out connections compared with configured capacity.")}
                  </CardDescription>
                </CardHeader>
                <CardContent className="flex flex-col gap-4">
                  <PoolRow label={t("Control plane")} pool={data.database.control_plane} />
                  <Separator />
                  <PoolRow label={t("Request log database")} pool={data.database.request_log} />
                </CardContent>
              </Card>
            </div>
        </>
      ) : null}
    </div>
  );
}
