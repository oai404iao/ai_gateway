import { useSearchParams } from "react-router";
import { useI18n } from "@/app/i18n";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { JsonViewer } from "@/components/shared/json-viewer";
import type { Column } from "@/components/shared/resource-table";
import { ResourceDirectory } from "@/features/admin/components/resource-directory";
import { useAuditLogs } from "@/features/admin/api";
import type { AuditLogView } from "@/api/types";
import { formatDateTime } from "@/lib/dates";

const LIMITS = [50, 100];

function objectLabel(log: AuditLogView): string {
  for (const snapshot of [log.after_redacted, log.before_redacted]) {
    if (!snapshot || typeof snapshot !== "object") continue;
    if ("display_name" in snapshot && typeof snapshot.display_name === "string" && snapshot.display_name.trim()) return snapshot.display_name;
    if ("name" in snapshot && typeof snapshot.name === "string" && snapshot.name.trim()) return snapshot.name;
  }
  return log.object_type;
}

export function AuditLogsPage() {
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  const limit = Number(params.get("limit")) === 50 ? 50 : 100;
  const { data, isLoading, error } = useAuditLogs(limit);

  const columns: Column<AuditLogView>[] = [
    {
      key: "occurred",
      header: t("Occurred"),
      render: (log) => formatDateTime(log.occurred_at),
    },
    { key: "action", header: t("Action"), render: (log) => <span className="font-mono text-xs">{log.action}</span> },
    { key: "object", header: t("Object"), render: objectLabel },
    { key: "actor", header: t("Actor"), render: (log) => log.actor_role ?? log.actor_type },
    {
      key: "correlation",
      header: t("Correlation"),
      render: (log) => (log.correlation_id ? log.correlation_id.slice(0, 8) : "—"),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Audit Logs"
        actions={
          <Select value={String(limit)} onValueChange={(value) => setParams((current) => {
            const next = new URLSearchParams(current);
            next.set("limit", String(value));
            next.delete("page");
            return next;
          }, { replace: true })}>
            <SelectTrigger aria-label={t("Result limit")} className="w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {LIMITS.map((value) => (
                  <SelectItem key={value} value={String(value)}>
                    {t("Last {count}", { count: value })}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        }
      />
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={data?.length === 0}
            emptyTitle="No audit events"
          >
            <ResourceDirectory
              columns={columns}
              rows={data ?? []}
              rowKey={(log) => log.id}
              rowName={(log) => `${log.action} · ${objectLabel(log)}`}
              searchText={(log) => [log.action, objectLabel(log), log.object_type, log.object_id, log.actor_role, log.actor_user_id, log.correlation_id].join(" ")}
              renderProperties={(selected) => <dl className="mt-3 grid gap-3 border-t pt-3">
              <DetailField label="Occurred" value={formatDateTime(selected.occurred_at)} />
              <DetailField label="Event ID" value={selected.id} mono />
              <DetailField label="Object type" value={selected.object_type} />
              <DetailField label="Object id" value={selected.object_id} mono />
              <DetailField label="Actor" value={selected.actor_role ?? selected.actor_type} />
              <DetailField label="Actor id" value={selected.actor_user_id ?? "—"} mono />
              <DetailField label="Correlation" value={selected.correlation_id ?? "—"} mono />
              <DetailField label="Reason" value={selected.reason ?? "—"} />
              <DetailField label="Before (redacted)" value={<JsonViewer value={selected.before_redacted} />} />
              <DetailField label="After (redacted)" value={<JsonViewer value={selected.after_redacted} />} />
            </dl>} />
          </AsyncResource>
    </div>
  );
}
