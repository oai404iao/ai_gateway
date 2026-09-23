import type { ReactNode } from "react";
import { useI18n } from "@/app/i18n";

export function ConfigurationSaveBar({
  dirty,
  children,
}: {
  dirty: boolean;
  children: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-background p-4">
      <span className="text-xs text-muted-foreground" role="status">
        {t(dirty ? "Unsaved changes" : "No pending edits")}
      </span>
      <div className="flex flex-wrap gap-2">
        {children}
      </div>
    </div>
  );
}
