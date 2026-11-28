import { useNavigate } from "react-router";
import { Plus } from "lucide-react";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { NavigationLink } from "@/components/shared/navigation-link";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";
import { useListPagination } from "@/lib/use-list-pagination";
import { ResourceDirectory } from "./resource-directory";

interface ListResult<T> {
  data: T[] | undefined;
  isLoading: boolean;
  error: unknown;
}

interface AdminListPageProps<T> {
  title: string;
  query: ListResult<T>;
  columns: Column<T>[];
  rowKey: (row: T) => string;
  detailPath: (row: T) => string;
  createLabel?: string;
  createPath?: string;
  headerActions?: React.ReactNode;
  backTo?: string;
  groupBy?: (row: T) => string;
  embedded?: boolean;
  linkColumnKey?: string;
  rowName?: (row: T) => string;
  searchText?: (row: T) => string;
}

export function AdminListPage<T>({
  title,
  query,
  columns,
  rowKey,
  detailPath,
  createLabel,
  createPath,
  headerActions,
  backTo,
  groupBy,
  embedded,
  linkColumnKey,
  rowName,
  searchText,
}: AdminListPageProps<T>) {
  const navigate = useNavigate();
  const origin = usePageOrigin();
  const pagination = useListPagination();
  const href = (row: T) => withReturnTo(detailPath(row), origin);
  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageHeader
        backTo={backTo}
        title={title}
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
            emptyTitle="No records"
          >
            {rowName ? <ResourceDirectory rows={query.data ?? []} columns={columns} rowKey={rowKey} rowName={rowName}
              searchText={searchText} rowHref={href} linkColumnKey={linkColumnKey} groupBy={groupBy} /> : <ResourceTable
              columns={columns}
              rows={query.data ?? []}
              rowKey={rowKey}
              rowHref={href}
              linkColumnKey={linkColumnKey}
              onRowClick={(row) => navigate(href(row))}
              groupBy={groupBy}
              pagination={pagination}
            />}
          </AsyncResource>
    </div>
  );
}
