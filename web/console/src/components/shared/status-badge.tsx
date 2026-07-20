import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

type Variant = "default" | "secondary" | "destructive";

interface StatusBadgeProps {
  value: string | boolean | null | undefined;
  /** Map a raw string to a display label. */
  label?: string;
  /** Map a raw string to a badge variant. */
  variant?: Variant;
  className?: string;
}

/** Renders an enabled/disabled or active/disabled/revoked status as a badge. */
export function StatusBadge({ value, label, variant, className }: StatusBadgeProps) {
  const text = label ?? statusLabel(value);
  const resolved = variant ?? statusVariant(value);
  return (
    <Badge variant={resolved} className={cn("capitalize", className)}>
      {text}
    </Badge>
  );
}

function statusLabel(value: string | boolean | null | undefined): string {
  if (value === true) return "enabled";
  if (value === false) return "disabled";
  if (value === null || value === undefined) return "—";
  return value;
}

function statusVariant(value: string | boolean | null | undefined): Variant {
  if (value === true) return "secondary";
  if (value === false) return "destructive";
  if (typeof value !== "string") return "default";
  if (value === "active" || value === "enabled") return "secondary";
  if (value === "revoked" || value === "disabled" || value === "invited") {
    return "destructive";
  }
  return "default";
}
