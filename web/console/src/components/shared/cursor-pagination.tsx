import { useId } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface CursorPaginationProps {
  page: number;
  pageSize: number;
  pageSizeOptions?: readonly number[];
  onPageSizeChange?: (pageSize: number) => void;
  hasPrevious: boolean;
  hasNext: boolean;
  onPrevious: () => void;
  onNext: () => void;
  itemCount: number;
  total?: number | null;
  isFetching?: boolean;
}

export function CursorPagination({
  page,
  pageSize,
  pageSizeOptions,
  onPageSizeChange,
  hasPrevious,
  hasNext,
  onPrevious,
  onNext,
  itemCount,
  total,
  isFetching = false,
}: CursorPaginationProps) {
  const selectId = useId();
  const { t } = useI18n();

  if (!hasPrevious && !hasNext && itemCount === 0 && total == null) {
    return null;
  }

  return (
    <div className="flex flex-col gap-3 pt-2 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex flex-wrap items-center gap-4">
        <p className="text-sm text-muted-foreground">
          {total != null
            ? t("Page {page} · {total} total", { page, total })
            : t("Page {page}", { page })}
        </p>
        {pageSizeOptions && onPageSizeChange ? (
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
        ) : null}
      </div>

      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!hasPrevious || isFetching}
          aria-label={t("Go to previous page")}
          onClick={onPrevious}
        >
          <ChevronLeft data-icon="inline-start" />
          {t("Previous")}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!hasNext || isFetching}
          aria-label={t("Go to next page")}
          onClick={onNext}
        >
          {t("Next")}
          <ChevronRight data-icon="inline-end" />
        </Button>
      </div>
    </div>
  );
}
