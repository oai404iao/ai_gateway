import { useNavigate } from "react-router";
import { Plus } from "lucide-react";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useI18n } from "@/app/i18n";
import { NavigationLink } from "@/components/shared/navigation-link";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";
import { useListPagination } from "@/lib/use-list-pagination";

interface ListResult<T> {
  data: T[] | undefined;
  isLoading: boolean;
  error: unknown;
}

interface AdminListPageProps<T> {
  title: string;
  description: string;
  query: ListResult<T>;
  columns: Column<T>[];
  rowKey: (row: T) => string;
  detailPath: (row: T) => string;
  createLabel?: string;
  createPath?: string;
  headerActions?: React.ReactNode;
  groupBy?: (row: T) => string;
  embedded?: boolean;
  linkColumnKey?: string;
}

export function AdminListPage<T>({
  title,
  description,
  query,
  columns,
  rowKey,
  detailPath,
  createLabel,
  createPath,
  headerActions,
  groupBy,
  embedded,
  linkColumnKey,
}: AdminListPageProps<T>) {
  const navigate = useNavigate();
  const { t } = useI18n();
  const origin = usePageOrigin();
  const pagination = useListPagination();
  const href = (row: T) => withReturnTo(detailPath(row), origin);
  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageHeader
        title={title}
        description={description}
        embedded={embedded}
        actions={
          headerActions || (createPath && createLabel) ? (
            <>
              {headerActions}
              {createPath && createLabel ? (
                <NavigationLink to={withReturnTo(createPath, origin)} variant="default" size="default">
                  <Plus data-icon="inline-start" /> {createLabel}
                </NavigationLink>
              ) : null}
            </>
          ) : undefined
        }
      />
          <AsyncResource
            isLoading={query.isLoading}
            error={query.error}
            isEmpty={query.data?.length === 0}
            emptyTitle={t("No records")}
            emptyDescription={t("There are no records to show yet.")}
          >
            <ResourceTable
              columns={columns}
              rows={query.data ?? []}
              rowKey={rowKey}
              rowHref={href}
              linkColumnKey={linkColumnKey}
              onRowClick={(row) => navigate(href(row))}
              groupBy={groupBy}
              pagination={pagination}
            />
          </AsyncResource>
    </div>
  );
}
