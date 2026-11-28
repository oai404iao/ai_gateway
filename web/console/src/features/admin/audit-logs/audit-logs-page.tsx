import { useEffect, useState } from "react";
import { useSearchParams } from "react-router";
import { useI18n } from "@/app/i18n";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { JsonViewer } from "@/components/shared/json-viewer";
import type { Column } from "@/components/shared/resource-table";
import { ResourceDirectory } from "@/features/admin/components/resource-directory";
import { useAuditLogs } from "@/features/admin/api";
import { useDebouncedValue } from "@/hooks/use-debounced-value";
import { useCursorPagination } from "@/lib/use-cursor-pagination";
import type { AuditLogView } from "@/api/types";
import { formatDateTime } from "@/lib/dates";

const LIMITS = [25, 50, 100] as const;

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
  const [search, setSearch] = useState(() => params.get("q") ?? "");
  const debouncedSearch = useDebouncedValue(search, 300);
  const trimmedSearch = debouncedSearch.trim();
  const limitParam = Number(params.get("limit"));
  const limit = LIMITS.includes(limitParam as (typeof LIMITS)[number]) ? limitParam : 50;
  const cursorPagination = useCursorPagination();
  const resetCursor = cursorPagination.reset;

  useEffect(() => {
    resetCursor();
  }, [trimmedSearch, limit, resetCursor]);

  const { data, isLoading, isFetching, error } = useAuditLogs({
    limit,
    cursor: cursorPagination.cursor,
    q: trimmedSearch || undefined,
  });
  const rows = data?.items ?? [];

  const handleSearchChange = (value: string) => {
    setSearch(value);
    setParams((current) => {
      const next = new URLSearchParams(current);
      if (value) next.set("q", value); else next.delete("q");
      return next;
    }, { replace: true });
  };

  const handlePageSizeChange = (nextLimit: number) => {
    setParams((current) => {
      const next = new URLSearchParams(current);
      if (nextLimit === 50) next.delete("limit"); else next.set("limit", String(nextLimit));
      return next;
    }, { replace: true });
  };

  const columns: Column<AuditLogView>[] = [
    {
      key: "occurred", basis: 7.5,
      header: t("Occurred"),
      render: (log) => formatDateTime(log.occurred_at),
    },
    { key: "action", basis: 7, header: t("Action"), render: (log) => <span className="font-mono text-xs">{log.action}</span> },
    { key: "object", basis: 9, header: t("Object"), render: objectLabel },
    { key: "actor", basis: 4, header: t("Actor"), render: (log) => log.actor_role ?? log.actor_type },
    {
      key: "correlation", basis: 5.5,
      header: t("Correlation"),
      render: (log) => (log.correlation_id ? log.correlation_id.slice(0, 8) : "—"),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Audit Logs" />
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={rows.length === 0 && !trimmedSearch}
            emptyTitle="No audit events"
          >
            <ResourceDirectory
              columns={columns}
              rows={rows}
              rowKey={(log) => log.id}
              rowName={(log) => `${log.action} · ${objectLabel(log)}`}
              searchText={(log) => [log.action, objectLabel(log), log.object_type, log.object_id, log.actor_role, log.actor_user_id, log.correlation_id].join(" ")}
              server={{
                search,
                onSearchChange: handleSearchChange,
                page: cursorPagination.page,
                pageSize: limit,
                pageSizeOptions: LIMITS,
                onPageSizeChange: handlePageSizeChange,
                hasPrevious: cursorPagination.hasPrevious,
                hasNext: Boolean(data?.next_cursor),
                onPrevious: cursorPagination.goPrevious,
                onNext: () => cursorPagination.goNext(data?.next_cursor),
                isFetching,
              }}
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
