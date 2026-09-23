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
import { Link } from "react-router";
import { Button } from "@/components/ui/button";

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
  rowHref?: (row: T) => string;
  linkColumnKey?: string;
  rowActionLabel?: string;
  empty?: ReactNode;
  groupBy?: (row: T) => string;
  pagination?:
    | false
    | {
        defaultPageSize?: number;
        pageSizeOptions?: readonly number[];
        page?: number;
        pageSize?: number;
        onPageChange?: (page: number) => void;
        onPageSizeChange?: (size: number) => void;
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
  rowHref,
  linkColumnKey = columns[0]?.key,
  rowActionLabel,
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
  const [localPage, setLocalPage] = useState(1);
  const [localPageSize, setLocalPageSize] = useState(defaultPageSize);
  const page = paginationOptions?.page ?? localPage;
  const pageSize = paginationOptions?.pageSize ?? localPageSize;
  const setPage = paginationOptions?.onPageChange ?? setLocalPage;
  const setPageSize = paginationOptions?.onPageSizeChange ?? setLocalPageSize;
  const pageCount = Math.max(1, Math.ceil(rows.length / pageSize));
  const showPagination =
    paginationEnabled && rows.length > Math.min(...pageSizeOptions);

  useEffect(() => {
    if (rows.length > 0 && page > pageCount) setPage(pageCount);
  }, [page, pageCount, rows.length, setPage]);

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
    <div className="flex min-w-0 flex-col gap-4">
      <div className="overflow-x-auto rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              {columns.map((column) => (
                <TableHead key={column.key} className={column.className}>
                  {column.header}
                </TableHead>
              ))}
              {rowActionLabel ? <TableHead><span className="sr-only">{rowActionLabel}</span></TableHead> : null}
            </TableRow>
          </TableHeader>
          <TableBody>
            {groupedRows.map((group) => (
              <Fragment key={group.key}>
                {group.label ? (
                  <TableRow>
                    <TableCell colSpan={columns.length + (rowActionLabel ? 1 : 0)} className="bg-muted/50">
                      <span className="flex items-center gap-2 font-medium">
                        {group.label}
                        <Badge variant="secondary">{rows.filter((row) => groupBy?.(row) === group.key).length}</Badge>
                      </span>
                    </TableCell>
                  </TableRow>
                ) : null}
                {group.rows.map((row) => (
                  <TableRow
                    key={rowKey(row)}
                    className={cn(onRowClick && "cursor-pointer")}
                    onClick={
                      onRowClick
                        ? (event) => {
                            const target =
                              event.target instanceof Element ? event.target : null;
                            const interactiveTarget = target?.closest(
                              "a, button, input, select, textarea, [role=button], [role=checkbox], [role=switch], [contenteditable=true], [data-row-click-ignore]",
                            );
                            if (
                              interactiveTarget &&
                              event.currentTarget.contains(interactiveTarget)
                            ) {
                              return;
                            }
                            onRowClick(row);
                          }
                        : undefined
                    }
                  >
                    {columns.map((column) => (
                      <TableCell key={column.key} className={column.className}>
                        {rowHref && column.key === linkColumnKey ? (
                          <Link to={rowHref(row)} className="rounded-sm font-medium underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring">
                            {column.render(row)}
                          </Link>
                        ) : column.render(row)}
                      </TableCell>
                    ))}
                    {rowActionLabel ? (
                      <TableCell>
                        <Button variant="ghost" size="sm" onClick={() => onRowClick?.(row)}>
                          {rowActionLabel}
                        </Button>
                      </TableCell>
                    ) : null}
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
            if (!paginationOptions?.onPageSizeChange) setPage(1);
          }}
        />
      ) : null}
    </div>
  );
}
