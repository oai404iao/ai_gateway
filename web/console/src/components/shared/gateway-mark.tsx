import type { SVGProps } from "react";
import { cn } from "@/lib/utils";

type GatewayMarkProps = SVGProps<SVGSVGElement>;

/**
 * Routing mark for ai-gateway.
 *
 * The angular outer stroke forms a gateway "G"; the accent stroke is the
 * request path entering the routing plane.
 */
export function GatewayMark({ className, ...props }: GatewayMarkProps) {
  return (
    <svg
      viewBox="0 0 32 32"
      fill="none"
      aria-hidden="true"
      className={cn("shrink-0", className)}
      {...props}
    >
      <path
        d="m23 9-3-2.5h-8.5L7 11v10l4.5 4.5H20l3-2.5"
        className="text-brand-foreground"
        stroke="currentColor"
        strokeWidth="4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M16.5 16H24v6.5"
        className="text-brand-accent"
        stroke="currentColor"
        strokeWidth="4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
