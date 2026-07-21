import { type MouseEvent, useId } from "react";
import { Field, FieldLabel } from "@/components/ui/field";
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from "@/components/ui/pagination";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

interface TablePaginationProps {
  page: number;
  pageCount: number;
  pageSize: number;
  totalItems: number;
  pageSizeOptions: readonly number[];
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
}

type PageToken = number | "start-ellipsis" | "end-ellipsis";

function pageTokens(page: number, pageCount: number): PageToken[] {
  if (pageCount <= 7) {
    return Array.from({ length: pageCount }, (_, index) => index + 1);
  }
  if (page <= 4) {
    return [1, 2, 3, 4, 5, "end-ellipsis", pageCount];
  }
  if (page >= pageCount - 3) {
    return [
      1,
      "start-ellipsis",
      pageCount - 4,
      pageCount - 3,
      pageCount - 2,
      pageCount - 1,
      pageCount,
    ];
  }
  return [1, "start-ellipsis", page - 1, page, page + 1, "end-ellipsis", pageCount];
}

export function TablePagination({
  page,
  pageCount,
  pageSize,
  totalItems,
  pageSizeOptions,
  onPageChange,
  onPageSizeChange,
}: TablePaginationProps) {
  const selectId = useId();
  const { t } = useI18n();
  const start = (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, totalItems);

  const changePage = (event: MouseEvent<HTMLAnchorElement>, nextPage: number) => {
    event.preventDefault();
    if (nextPage >= 1 && nextPage <= pageCount && nextPage !== page) {
      onPageChange(nextPage);
    }
  };

  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex flex-wrap items-center gap-4">
        <p className="text-sm text-muted-foreground">
          {t("Showing {start}-{end} of {total}", { start, end, total: totalItems })}
        </p>
        <Field orientation="horizontal" className="w-auto">
          <FieldLabel htmlFor={selectId} className="text-muted-foreground">
            {t("Rows per page")}
          </FieldLabel>
          <Select
            value={String(pageSize)}
            onValueChange={(value) => onPageSizeChange(Number(value))}
          >
            <SelectTrigger id={selectId} size="sm" className="w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {pageSizeOptions.map((option) => (
                  <SelectItem key={option} value={String(option)}>
                    {option}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>
      </div>

      <Pagination className="mx-0 w-auto justify-start sm:justify-end">
        <PaginationContent>
          <PaginationItem>
            <PaginationPrevious
              href="#"
              text={t("Previous")}
              aria-label={t("Go to previous page")}
              aria-disabled={page === 1}
              tabIndex={page === 1 ? -1 : undefined}
              className={cn(page === 1 && "pointer-events-none opacity-50")}
              onClick={(event) => changePage(event, page - 1)}
            />
          </PaginationItem>
          {pageTokens(page, pageCount).map((token) =>
            typeof token === "number" ? (
              <PaginationItem key={token}>
                <PaginationLink
                  href="#"
                  isActive={token === page}
                  aria-label={t("Go to page {page}", { page: token })}
                  onClick={(event) => changePage(event, token)}
                >
                  {token}
                </PaginationLink>
              </PaginationItem>
            ) : (
              <PaginationItem key={token}>
                <PaginationEllipsis />
              </PaginationItem>
            ),
          )}
          <PaginationItem>
            <PaginationNext
              href="#"
              text={t("Next")}
              aria-label={t("Go to next page")}
              aria-disabled={page === pageCount}
              tabIndex={page === pageCount ? -1 : undefined}
              className={cn(page === pageCount && "pointer-events-none opacity-50")}
              onClick={(event) => changePage(event, page + 1)}
            />
          </PaginationItem>
        </PaginationContent>
      </Pagination>
    </div>
  );
}
