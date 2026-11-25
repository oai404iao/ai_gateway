import { GatewayMark } from "@/components/shared/gateway-mark";
import { cn } from "@/lib/utils";

export function Brand({ compact = false, className }: { compact?: boolean; className?: string }) {
  return (
    <span className={cn("inline-flex shrink-0 items-center gap-2 font-semibold tracking-tight whitespace-nowrap", compact ? "text-base" : "text-lg", className)}>
      <span className="grid size-7 place-items-center rounded-[7px] bg-primary"><GatewayMark className="size-6" /></span>
      AI Gateway
    </span>
  );
}
