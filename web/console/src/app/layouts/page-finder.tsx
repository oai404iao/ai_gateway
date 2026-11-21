import { useRef } from "react";
import { useNavigate } from "react-router";
import { Combobox } from "@base-ui/react/combobox";
import { CornerDownLeft, Search } from "lucide-react";
import { Dialog, DialogClose, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useI18n } from "@/app/i18n";
import { finderPages } from "./nav";

export function PageFinder({
  admin,
  open,
  onOpenChange,
}: {
  admin: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const input = useRef<HTMLInputElement>(null);
  const navigating = useRef(false);
  const items = finderPages(admin).map(({ item, parents }) => ({
    ...item,
    group: parents.map((label) => t(label)).join(" / "),
    text: t(item.label),
  }));
  type Match = (typeof items)[number];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        initialFocus={() => {
          navigating.current = false;
          return input.current;
        }}
        finalFocus={() => !navigating.current}
        showCloseButton={false}
        className="top-[min(15dvh,8rem)] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-lg"
      >
        <DialogTitle className="sr-only">{t("Find page")}</DialogTitle>
        <Combobox.Root<Match>
          items={items}
          inline
          autoHighlight
          itemToStringLabel={(item) => item.text}
          filter={(item, query) =>
            [item.text, item.group, item.label, item.path].some((value) =>
              value.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
            )
          }
          onValueChange={(item) => {
            if (item) {
              navigating.current = item.path !== window.location.pathname;
              onOpenChange(false);
              void navigate(item.path);
            }
          }}
        >
          <div className="flex items-center gap-2 border-b px-4 py-2">
            <Search aria-hidden="true" className="size-4 text-muted-foreground" />
            <Combobox.Input
              ref={input}
              render={
                <Input className="min-w-0 flex-1 border-0 bg-transparent shadow-none focus-visible:ring-0" />
              }
              aria-label={t("Find page")}
              placeholder={t("Search pages…")}
            />
            <DialogClose render={<Button variant="ghost" size="xs" aria-label={t("Close")} />}>
              Esc
            </DialogClose>
          </div>
          <Combobox.Empty className="p-4 text-sm text-muted-foreground empty:p-0">
            {t("No matching pages")}
          </Combobox.Empty>
          <Combobox.List className="max-h-[50dvh] overflow-y-auto overscroll-contain p-1.5 empty:hidden">
            {(item: Match) => (
              <Combobox.Item
                key={item.path}
                value={item}
                className="group flex cursor-pointer items-center gap-2 rounded-md px-3 py-2 text-sm data-highlighted:bg-accent data-highlighted:text-accent-foreground"
              >
                <item.icon aria-hidden="true" className="size-4" /> {item.text}
                <span
                  className="ml-auto max-w-[42%] truncate text-xs text-muted-foreground"
                  title={item.group}
                >
                  {item.group}
                </span>
                <CornerDownLeft
                  aria-hidden="true"
                  className="size-3 opacity-0 group-data-highlighted:opacity-70"
                />
              </Combobox.Item>
            )}
          </Combobox.List>
        </Combobox.Root>
      </DialogContent>
    </Dialog>
  );
}
