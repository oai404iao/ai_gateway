import { useNavigate } from "react-router";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";

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
}: AdminListPageProps<T>) {
  const navigate = useNavigate();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={title}
        description={description}
        actions={
          onCreate && createLabel ? (
            <Button onClick={onCreate}>
              <Plus data-icon="inline-start" /> {createLabel}
            </Button>
          ) : undefined
        }
      />
      <Card>
        <CardHeader>
          <CardTitle>{title}</CardTitle>
          <CardDescription>Click a row to view or edit.</CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={query.isLoading}
            error={query.error}
            isEmpty={query.data?.length === 0}
            emptyTitle="No records"
            emptyDescription="There are no records to show yet."
          >
            <ResourceTable
              columns={columns}
              rows={query.data ?? []}
              rowKey={rowKey}
              onRowClick={(row) => navigate(detailPath(row))}
            />
          </AsyncResource>
        </CardContent>
      </Card>
    </div>
  );
}
