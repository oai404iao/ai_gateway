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
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { JsonViewer } from "@/components/shared/json-viewer";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useAuditLogs } from "@/features/admin/api";
import type { AuditLogView } from "@/api/types";
import { formatDateTime, formatRelative } from "@/lib/dates";

const LIMITS = [50, 100];

export function AuditLogsPage() {
  const [limit, setLimit] = useState(100);
  const { data, isLoading, error } = useAuditLogs(limit);
  const [selected, setSelected] = useState<AuditLogView | null>(null);

  const columns: Column<AuditLogView>[] = [
    {
      key: "occurred",
      header: "Occurred",
      render: (log) => (
        <span className="flex flex-col">
          <span>{formatDateTime(log.occurred_at)}</span>
          <span className="text-xs text-muted-foreground">{formatRelative(log.occurred_at)}</span>
        </span>
      ),
    },
    { key: "action", header: "Action", render: (log) => <span className="font-mono text-xs">{log.action}</span> },
    { key: "object", header: "Object", render: (log) => `${log.object_type}:${log.object_id.slice(0, 8)}` },
    { key: "actor", header: "Actor", render: (log) => log.actor_role ?? log.actor_type },
    {
      key: "correlation",
      header: "Correlation",
      render: (log) => (log.correlation_id ? log.correlation_id.slice(0, 8) : "—"),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Audit Logs"
        description="Control-plane mutations and administrative actions."
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
          <CardTitle>Events</CardTitle>
          <CardDescription>Before/after payloads are redacted by the gateway.</CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={data?.length === 0}
            emptyTitle="No audit events"
            emptyDescription="There are no audited events yet."
          >
            <ResourceTable
              columns={columns}
              rows={data ?? []}
              rowKey={(log) => log.id}
              onRowClick={(log) => setSelected(log)}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      <Sheet open={Boolean(selected)} onOpenChange={(open) => !open && setSelected(null)}>
        <SheetContent className="sm:max-w-md overflow-y-auto">
          <SheetHeader>
            <SheetTitle>{selected?.action}</SheetTitle>
            <SheetDescription>
              {selected ? formatDateTime(selected.occurred_at) : ""}
            </SheetDescription>
          </SheetHeader>
          {selected ? (
            <dl className="grid grid-cols-1 gap-3 p-4">
              <DetailField label="Object type" value={selected.object_type} />
              <DetailField label="Object id" value={selected.object_id} mono />
              <DetailField label="Actor" value={selected.actor_role ?? selected.actor_type} />
              <DetailField label="Actor id" value={selected.actor_user_id ?? "—"} mono />
              <DetailField label="Correlation" value={selected.correlation_id ?? "—"} mono />
              <DetailField label="Reason" value={selected.reason ?? "—"} />
              <DetailField label="Before (redacted)" value={<JsonViewer value={selected.before_redacted} />} />
              <DetailField label="After (redacted)" value={<JsonViewer value={selected.after_redacted} />} />
            </dl>
          ) : null}
        </SheetContent>
      </Sheet>
    </div>
  );
}
