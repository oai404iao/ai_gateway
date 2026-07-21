import { Fragment, type ReactNode, useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { TablePagination } from "@/components/shared/table-pagination";
import { cn } from "@/lib/utils";

export interface Column<T> {
  key: string;
  header: string;
  render: (row: T) => ReactNode;
  className?: string;
}

interface ResourceTableProps<T> {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  onRowClick?: (row: T) => void;
  empty?: ReactNode;
  groupBy?: (row: T) => string;
  pagination?:
    | false
    | {
        defaultPageSize?: number;
        pageSizeOptions?: readonly number[];
      };
}

const DEFAULT_PAGE_SIZE = 20;
const DEFAULT_PAGE_SIZE_OPTIONS = [10, 20, 50] as const;

/** A compact, clickable, grouped, and paginated data table used by list pages. */
export function ResourceTable<T>({
  columns,
  rows,
  rowKey,
  onRowClick,
  empty,
  groupBy,
  pagination,
}: ResourceTableProps<T>) {
  const paginationEnabled = pagination !== false;
  const paginationOptions = pagination === false ? undefined : pagination;
  const pageSizeOptions =
    paginationOptions?.pageSizeOptions ?? DEFAULT_PAGE_SIZE_OPTIONS;
  const defaultPageSize =
    paginationOptions?.defaultPageSize ?? DEFAULT_PAGE_SIZE;
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(defaultPageSize);
  const pageCount = Math.max(1, Math.ceil(rows.length / pageSize));
  const showPagination =
    paginationEnabled && rows.length > Math.min(...pageSizeOptions);

  useEffect(() => {
    if (page > pageCount) setPage(pageCount);
  }, [page, pageCount]);

  const visibleRows = useMemo(() => {
    if (!paginationEnabled) return rows;
    const start = (page - 1) * pageSize;
    return rows.slice(start, start + pageSize);
  }, [page, pageSize, paginationEnabled, rows]);

  const groupedRows = useMemo(() => {
    if (!groupBy) return [{ key: "", label: null, rows: visibleRows }];
    const groups = new Map<string, T[]>();
    for (const row of visibleRows) {
      const group = groupBy(row);
      groups.set(group, [...(groups.get(group) ?? []), row]);
    }
    return [...groups.entries()].map(([key, grouped]) => ({
      key,
      label: key,
      rows: grouped,
    }));
  }, [groupBy, visibleRows]);

  if (rows.length === 0 && empty) {
    return <div>{empty}</div>;
  }
  return (
    <div className="flex flex-col gap-4">
      <div className="overflow-x-auto rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              {columns.map((column) => (
                <TableHead key={column.key} className={column.className}>
                  {column.header}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {groupedRows.map((group) => (
              <Fragment key={group.key}>
                {group.label ? (
                  <TableRow>
                    <TableCell colSpan={columns.length} className="bg-muted/50">
                      <span className="flex items-center gap-2 font-medium">
                        {group.label}
                        <Badge variant="secondary">{group.rows.length}</Badge>
                      </span>
                    </TableCell>
                  </TableRow>
                ) : null}
                {group.rows.map((row) => (
                  <TableRow
                    key={rowKey(row)}
                    className={cn(onRowClick && "cursor-pointer")}
                    onClick={onRowClick ? () => onRowClick(row) : undefined}
                  >
                    {columns.map((column) => (
                      <TableCell key={column.key} className={column.className}>
                        {column.render(row)}
                      </TableCell>
                    ))}
                  </TableRow>
                ))}
              </Fragment>
            ))}
          </TableBody>
        </Table>
      </div>
      {showPagination ? (
        <TablePagination
          page={page}
          pageCount={pageCount}
          pageSize={pageSize}
          totalItems={rows.length}
          pageSizeOptions={pageSizeOptions}
          onPageChange={setPage}
          onPageSizeChange={(nextPageSize) => {
            setPageSize(nextPageSize);
            setPage(1);
          }}
        />
      ) : null}
    </div>
  );
}
