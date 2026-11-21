import { useEffect, type RefObject } from "react";
import { useBlocker } from "react-router";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { useI18n } from "@/app/i18n";

export function ConfigurationDraftBlocker({
  dirty,
  saving,
  bypass,
  directorySearchParam,
}: {
  dirty: boolean;
  saving: boolean;
  bypass: RefObject<boolean>;
  directorySearchParam?: string;
}) {
  const { t } = useI18n();
  const editorSearch = (search: string) => {
    const params = new URLSearchParams(search);
    if (directorySearchParam) params.delete(directorySearchParam);
    return params.toString();
  };
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      (dirty || saving) &&
      !bypass.current &&
      (currentLocation.pathname !== nextLocation.pathname ||
        (directorySearchParam
          ? editorSearch(currentLocation.search) !== editorSearch(nextLocation.search)
          : currentLocation.search !== nextLocation.search) ||
        currentLocation.hash !== nextLocation.hash),
  );
  useEffect(() => {
    if (!dirty && !saving && blocker.state === "blocked") blocker.reset();
  }, [dirty, saving, blocker]);
  return (
    <ConfirmDialog
      open={blocker.state === "blocked"}
      onOpenChange={(open) => {
        if (!open && blocker.state === "blocked") blocker.reset();
      }}
      title={t("Discard unsaved changes?")}
      description={t("Your edits on this page will be lost.")}
      confirmLabel={t("Discard changes")}
      confirmDisabled={saving}
      finalFocus={() => !bypass.current}
      destructive
      onConfirm={() => {
        if (blocker.state === "blocked") {
          bypass.current = true;
          blocker.proceed();
        }
      }}
    />
  );
}
