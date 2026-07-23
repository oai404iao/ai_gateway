import { useNavigate } from "react-router";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useI18n } from "@/app/i18n";

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
  onCreate?: () => void;
  headerActions?: React.ReactNode;
  groupBy?: (row: T) => string;
}

export function AdminListPage<T>({
  title,
  description,
  query,
  columns,
  rowKey,
  detailPath,
  createLabel,
  onCreate,
  headerActions,
  groupBy,
}: AdminListPageProps<T>) {
  const navigate = useNavigate();
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={title}
        description={description}
        actions={
          headerActions || (onCreate && createLabel) ? (
            <>
              {headerActions}
              {onCreate && createLabel ? (
                <Button onClick={onCreate}>
                  <Plus data-icon="inline-start" /> {createLabel}
                </Button>
              ) : null}
            </>
          ) : undefined
        }
      />
      <Card>
        <CardHeader>
          <CardTitle>{t(title)}</CardTitle>
          <CardDescription>{t("Click a row to view or edit.")}</CardDescription>
        </CardHeader>
        <CardContent>
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
              onRowClick={(row) => navigate(detailPath(row))}
              groupBy={groupBy}
            />
          </AsyncResource>
        </CardContent>
      </Card>
    </div>
  );
}
