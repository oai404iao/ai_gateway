import { Badge } from "@/components/ui/badge";
import { translate } from "@/app/i18n";
import { cn } from "@/lib/utils";

type Variant = "default" | "secondary" | "destructive" | "success" | "warning" | "info";

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
    <Badge variant={resolved} className={cn(className)}>
      {text}
    </Badge>
  );
}

function statusLabel(value: string | boolean | null | undefined): string {
  if (value === true) return translate("Enabled");
  if (value === false) return translate("Disabled");
  if (value === null || value === undefined) return "—";
  switch (value) {
    case "active":
      return translate("Active");
    case "enabled":
      return translate("Enabled");
    case "disabled":
      return translate("Disabled");
    case "revoked":
      return translate("Revoked");
    case "suspended":
      return translate("Suspended");
    case "invited":
      return translate("Invited");
    case "succeeded":
      return translate("Succeeded");
    case "failed":
      return translate("Failed");
    case "rejected":
      return translate("Rejected");
    case "cancelled":
      return translate("Cancelled");
    case "admin":
      return translate("Administrator");
    case "user":
      return translate("User");
    default:
      return value;
  }
}

function statusVariant(value: string | boolean | null | undefined): Variant {
  if (value === true) return "success";
  if (value === false) return "destructive";
  if (typeof value !== "string") return "default";
  if (value === "active" || value === "enabled" || value === "succeeded") return "success";
  if (value === "suspended" || value === "invited" || value === "rejected") return "warning";
  if (value === "revoked" || value === "disabled" || value === "failed") {
    return "destructive";
  }
  if (value === "admin") return "info";
  return "default";
}
