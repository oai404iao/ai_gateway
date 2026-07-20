import { useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";

interface JsonViewerProps {
  value: unknown;
  className?: string;
}

/** Minimal collapsible JSON viewer for audit-log before/after payloads. */
export function JsonViewer({ value, className }: JsonViewerProps) {
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
    <div className={className}>
      <button
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
      >
        <ChevronRight
          className={cn("size-3 transition-transform", open && "rotate-90")}
        />
        {open ? "collapse" : "expand"}
      </button>
      {open ? (
        <pre className="mt-1 max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
          {text}
        </pre>
      ) : null}
    </div>
  );
}
