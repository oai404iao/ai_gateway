import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/app/i18n";

export function ConfigurationSaveBar({
  dirty,
  saving,
  onCancel,
  children,
}: {
  dirty: boolean;
  saving: boolean;
  onCancel: () => void;
  children: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <div className="sticky bottom-3 flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-background p-3 shadow-sm xl:col-span-2">
      <span className="text-xs text-muted-foreground" role="status">
        {t(dirty ? "Unsaved changes" : "No pending edits")}
      </span>
      <div className="flex flex-wrap gap-2">
        <Button variant="ghost" disabled={saving} onClick={onCancel}>
          {t("Cancel")}
        </Button>
        {children}
      </div>
    </div>
  );
}
