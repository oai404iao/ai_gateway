import { cn } from "@/lib/utils";

interface DetailFieldProps {
  label: string;
  value: React.ReactNode;
  mono?: boolean;
  className?: string;
}

/** A label/value row used on read-only detail pages. */
export function DetailField({ label, value, mono, className }: DetailFieldProps) {
  return (
    <div className={cn("flex flex-col gap-1", className)}>
      <dt className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {label}
      </dt>
      <dd className={cn("text-sm break-words", mono && "font-mono")}>{value}</dd>
    </div>
  );
}
