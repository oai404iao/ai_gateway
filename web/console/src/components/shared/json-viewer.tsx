import { useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import { motion } from "motion/react";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useI18n } from "@/app/i18n";
import { cn } from "@/lib/utils";

interface JsonViewerProps {
  value: unknown;
  className?: string;
}

const MotionChevron = motion.create(ChevronRight);

/** Minimal collapsible JSON viewer for audit-log before/after payloads. */
export function JsonViewer({ value, className }: JsonViewerProps) {
  const { t } = useI18n();
  const text = useMemo(() => {
    try {
      return JSON.stringify(value ?? null, null, 2);
    } catch {
      return String(value);
    }
  }, [value]);
  const [open, setOpen] = useState(false);
  const isEmpty = text === "{}" || text === "null" || text === "[]";

  if (isEmpty) {
    return <span className={cn("text-muted-foreground", className)}>{text}</span>;
  }

  return (
    <Collapsible open={open} onOpenChange={setOpen} className={className}>
      <CollapsibleTrigger render={<Button variant="ghost" size="xs" />}>
        <MotionChevron
          data-icon="inline-start"
          initial={false}
          animate={{ rotate: open ? 90 : 0 }}
        />
        {open ? t("Collapse") : t("Expand")}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ScrollArea className="mt-1 max-h-80 rounded-md border bg-muted/40">
          <pre className="p-3 font-mono text-xs">{text}</pre>
        </ScrollArea>
      </CollapsibleContent>
    </Collapsible>
  );
}
