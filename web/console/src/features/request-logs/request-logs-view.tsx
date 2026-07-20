import { useState } from "react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Badge } from "@/components/ui/badge";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useRequestLog } from "@/features/request-logs/api";
import type { RequestLogView } from "@/api/types";
import { formatDateTime, formatRelative } from "@/lib/dates";
import { formatCurrency, formatDurationMs, formatTokens } from "@/lib/formatters";
import { apiFormatLabel, outcomeLabel, outcomeVariant } from "@/lib/permissions";

const LIMITS = [25, 50, 100];

interface RequestLogListResult {
  data: RequestLogView[] | undefined;
  isLoading: boolean;
  error: unknown;
}

type UseRequestLogs = (limit: number) => RequestLogListResult;

interface RequestLogsViewProps {
  title: string;
  description: string;
  basePath: string;
  useLogs: UseRequestLogs;
}

export function RequestLogsView({ title, description, basePath, useLogs }: RequestLogsViewProps) {
  const [limit, setLimit] = useState(50);
  const query = useLogs(limit);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const detail = useRequestLog(basePath, selectedId);

  const columns: Column<RequestLogView>[] = [
    {
      key: "started",
      header: "Started",
      render: (log) => (
        <span className="flex flex-col">
          <span>{formatDateTime(log.started_at)}</span>
          <span className="text-xs text-muted-foreground">{formatRelative(log.started_at)}</span>
        </span>
      ),
    },
    {
      key: "model",
      header: "Model",
      render: (log) => (
        <span className="flex flex-col">
          <span className="font-medium">{log.client_model}</span>
          <span className="text-xs text-muted-foreground">{apiFormatLabel(log.api_format)}</span>
        </span>
      ),
    },
    {
      key: "outcome",
      header: "Outcome",
      render: (log) => <Badge variant={outcomeVariant(log.outcome)}>{outcomeLabel(log.outcome)}</Badge>,
    },
    {
      key: "status",
      header: "HTTP",
      render: (log) => (log.response_status_code ?? "—"),
    },
    {
      key: "tokens",
      header: "Output tokens",
      render: (log) => formatTokens(log.output_tokens),
    },
    {
      key: "cost",
      header: "Cost",
      render: (log) => formatCurrency(log.cost_amount, log.currency),
    },
    {
      key: "duration",
      header: "Duration",
      render: (log) => formatDurationMs(log.total_duration_ms),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={title}
        description={description}
        actions={
          <Select value={String(limit)} onValueChange={(value) => setLimit(Number(value))}>
            <SelectTrigger className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {LIMITS.map((value) => (
                <SelectItem key={value} value={String(value)}>
                  Last {value}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />
      <Card>
        <CardHeader>
          <CardTitle>Requests</CardTitle>
          <CardDescription>The gateway never stores prompts or completions.</CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={query.isLoading}
            error={query.error}
            isEmpty={query.data?.length === 0}
            emptyTitle="No request logs"
            emptyDescription="There are no logged requests in this view yet."
          >
            <ResourceTable
              columns={columns}
              rows={query.data ?? []}
              rowKey={(log) => log.id}
              onRowClick={(log) => setSelectedId(log.id)}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      <Sheet open={Boolean(selectedId)} onOpenChange={(open) => !open && setSelectedId(null)}>
        <SheetContent className="sm:max-w-md overflow-y-auto">
          <SheetHeader>
            <SheetTitle>Request log</SheetTitle>
            <SheetDescription>
              {detail.data ? formatDateTime(detail.data.started_at) : "Loading…"}
            </SheetDescription>
          </SheetHeader>
          {detail.data ? (
            <dl className="grid grid-cols-1 gap-3 p-4">
              <DetailField label="Outcome" value={<Badge variant={outcomeVariant(detail.data.outcome)}>{outcomeLabel(detail.data.outcome)}</Badge>} />
              <DetailField label="HTTP status" value={detail.data.response_status_code ?? "—"} />
              <DetailField label="Streamed" value={detail.data.streamed ? "yes" : "no"} />
              <DetailField label="Client model" value={detail.data.client_model} mono />
              <DetailField label="Upstream model" value={detail.data.upstream_model ?? "—"} mono />
              <DetailField label="API format" value={apiFormatLabel(detail.data.api_format)} />
              <DetailField label="TTFT" value={formatDurationMs(detail.data.ttft_ms)} />
              <DetailField label="Total duration" value={formatDurationMs(detail.data.total_duration_ms)} />
              <DetailField label="Input tokens" value={formatTokens(detail.data.input_tokens)} />
              <DetailField label="Cached input" value={formatTokens(detail.data.cached_input_tokens)} />
              <DetailField label="Cache write" value={formatTokens(detail.data.cache_write_tokens)} />
              <DetailField label="Output tokens" value={formatTokens(detail.data.output_tokens)} />
              <DetailField label="Cost" value={formatCurrency(detail.data.cost_amount, detail.data.currency)} />
              <DetailField label="Billed at" value={formatDateTime(detail.data.billed_at)} />
              <DetailField label="Error code" value={detail.data.error_code ?? "—"} mono />
              <DetailField label="Channel group" value={detail.data.channel_group_id ?? "—"} mono />
              <DetailField label="Channel" value={detail.data.channel_id ?? "—"} mono />
              <DetailField label="Completed" value={formatDateTime(detail.data.completed_at)} />
            </dl>
          ) : null}
        </SheetContent>
      </Sheet>
    </div>
  );
}
