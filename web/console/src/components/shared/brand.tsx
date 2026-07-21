import { useI18n } from "@/app/i18n";
import { cn } from "@/lib/utils";

interface BrandProps {
  compact?: boolean;
  collapseInSidebar?: boolean;
  className?: string;
}

/** Shared Console brand mark; there is no shadcn primitive for product branding. */
export function Brand({
  compact = false,
  collapseInSidebar = false,
  className,
}: BrandProps) {
  const { t } = useI18n();

  return (
    <div
      className={cn(
        "flex items-center gap-2",
        collapseInSidebar && "group-data-[collapsible=icon]:justify-center",
        className,
      )}
    >
      <div
        className={cn(
          "flex items-center justify-center rounded-md bg-primary text-primary-foreground",
          compact ? "size-7" : "size-8",
        )}
        aria-hidden="true"
      >
        <span className={cn("font-bold", compact ? "text-xs" : "text-sm")}>AG</span>
      </div>
      <div
        className={cn(
          "flex flex-col",
          collapseInSidebar && "group-data-[collapsible=icon]:hidden",
        )}
      >
        <span className={cn("font-semibold", compact ? "text-sm" : "text-base")}>
          AI Gateway
        </span>
        <span className="text-xs text-muted-foreground">{t("Console")}</span>
      </div>
    </div>
  );
}
