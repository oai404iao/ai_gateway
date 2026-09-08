import { useCallback, useContext, useEffect, useRef, useState } from "react";
import {
  UNSAFE_DataRouterContext,
  useNavigate,
  type NavigateOptions,
} from "react-router";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { useI18n } from "@/app/i18n";
import { ConfigurationDraftBlocker } from "./configuration-draft-blocker";

/** Draft state is local only; secrets and form values never enter URLs or storage. */
export function useConfigurationDraft(
  saving: boolean,
  externalDirty?: boolean,
) {
  const rawNavigate = useNavigate();
  const { t } = useI18n();
  const dataRouter = useContext(UNSAFE_DataRouterContext) !== null;
  const bypass = useRef(false);
  const [localDirty, setDirty] = useState(false);
  const dirty = externalDirty ?? localDirty;
  const [pending, setPending] = useState<{
    to: string;
    options?: NavigateOptions;
  } | null>(null);
  const status = useRef({ dirty, dataRouter, saving });
  status.current = { dirty, dataRouter, saving };
  const markDirty = () => {
    bypass.current = false;
    setDirty(true);
  };
  const markSaved = () => {
    bypass.current = true;
    setDirty(false);
  };
  useEffect(() => {
    if (externalDirty) bypass.current = false;
  }, [externalDirty]);
  const navigate = useCallback(
    (to: string, options?: NavigateOptions) => {
      const { dirty, dataRouter, saving } = status.current;
      if (saving && !bypass.current && !dataRouter) return;
      if (dirty && !bypass.current && !dataRouter) {
        if (!saving) setPending({ to, options });
      } else {
        void rawNavigate(to, options);
      }
    },
    [rawNavigate],
  );
  useEffect(() => {
    if (!dirty && !saving) return;
    const unload = (event: BeforeUnloadEvent) => {
      if (!bypass.current) {
        event.preventDefault();
        event.returnValue = "";
      }
    };
    const anchorClick = (event: MouseEvent) => {
      if (
        dataRouter ||
        bypass.current ||
        event.defaultPrevented ||
        event.button !== 0 ||
        event.ctrlKey ||
        event.metaKey ||
        event.altKey ||
        event.shiftKey
      )
        return;
      const anchor =
        event.target instanceof Element
          ? event.target.closest("a[href]")
          : null;
      if (
        !(anchor instanceof HTMLAnchorElement) ||
        anchor.hasAttribute("download") ||
        (anchor.target && anchor.target !== "_self")
      )
        return;
      const url = new URL(anchor.href);
      if (url.origin !== window.location.origin) return;
      event.preventDefault();
      if (!saving)
        setPending({ to: `${url.pathname}${url.search}${url.hash}` });
    };
    window.addEventListener("beforeunload", unload);
    document.addEventListener("click", anchorClick, true);
    return () => {
      window.removeEventListener("beforeunload", unload);
      document.removeEventListener("click", anchorClick, true);
    };
  }, [dataRouter, dirty, saving]);
  const navigationGuard = dataRouter ? (
    <ConfigurationDraftBlocker dirty={dirty} saving={saving} bypass={bypass} />
  ) : (
    <ConfirmDialog
      open={pending !== null}
      onOpenChange={(open) => {
        if (!open) setPending(null);
      }}
      title={t("Discard unsaved changes?")}
      description={t("Your edits on this page will be lost.")}
      confirmLabel={t("Discard changes")}
      confirmDisabled={saving}
      destructive
      onConfirm={() => {
        bypass.current = true;
        if (pending) void rawNavigate(pending.to, pending.options);
        setPending(null);
      }}
    />
  );
  return { dirty, markDirty, markSaved, navigate, navigationGuard };
}
