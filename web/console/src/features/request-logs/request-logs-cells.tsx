import { type ReactNode, type RefObject } from "react";
import { ArrowDown, ArrowUp, PanelRight, X } from "lucide-react";
import type { RequestLogView } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatDurationMs, formatExactTokens, formatTokens, formatUsd } from "@/lib/formatters";
import { cn } from "@/lib/utils";

function tokenRemainder(
  total: number | null | undefined,
  subset: number | null | undefined,
): number | null {
  if (total === null || total === undefined) return null;
  return Math.max(0, total - (subset ?? 0));
}

export function requestProtocolLabel(
  protocol: RequestLogView["request_protocol"],
  t: (value: string) => string,
) {
  switch (protocol) {
    case "non_stream":
      return t("Non-stream");
    case "sse":
      return "SSE";
    case "websocket":
      return t("WebSocket");
  }
}

export function TruncatedName({ value }: { value: string | null }) {
  if (!value) return "—";
  return (
    <Tooltip disableHoverablePopup>
      <TooltipTrigger
        render={
          <span
            tabIndex={0}
            className="block truncate outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        }
      >
        {value}
      </TooltipTrigger>
      <TooltipContent className="break-words">{value}</TooltipContent>
    </Tooltip>
  );
}

export function TokenUsage({ log }: { log: RequestLogView }) {
  const { t } = useI18n();
  const uncachedInput = tokenRemainder(log.input_tokens, log.cached_input_tokens);
  const uncachedInputLabel = `${t("Uncached input")}: ${formatExactTokens(uncachedInput)}`;
  const cachedInputLabel = `${t("Cached input")}: ${formatExactTokens(log.cached_input_tokens)}`;
  const outputLabel = `${t("Output tokens")}: ${formatExactTokens(log.output_tokens)}`;
  const reasoningLabel = `${t("Reasoning tokens")}: ${formatExactTokens(log.reasoning_tokens)}`;

  return (
    <span className="flex flex-col gap-1.5 tabular-nums">
      <span className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
        <span
          className="flex max-w-full items-center gap-1"
          aria-label={uncachedInputLabel}
          title={uncachedInputLabel}
        >
          <ArrowUp className="size-4 shrink-0 text-primary" aria-hidden />
          <span className="truncate">{formatTokens(uncachedInput)}</span>
        </span>
        {log.cached_input_tokens !== null && log.cached_input_tokens > 0 ? (
          <Badge variant="success" className="max-w-full" aria-label={cachedInputLabel} title={cachedInputLabel}>
            <span className="truncate">{formatTokens(log.cached_input_tokens)}</span>
          </Badge>
        ) : null}
      </span>
      <span className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
        <span
          className="flex max-w-full items-center gap-1"
          aria-label={outputLabel}
          title={outputLabel}
        >
          <ArrowDown className="size-4 shrink-0 text-info" aria-hidden />
          <span className="truncate">{formatTokens(log.output_tokens)}</span>
        </span>
        {log.reasoning_tokens !== null && log.reasoning_tokens > 0 ? (
          <Badge variant="info" className="max-w-full" aria-label={reasoningLabel} title={reasoningLabel}>
            <span className="truncate">{formatTokens(log.reasoning_tokens)}</span>
          </Badge>
        ) : null}
      </span>
    </span>
  );
}

export function reasoningEffortLabel(value: string): string {
  if (value === "xhigh") return "XHigh";
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}

export function RequestModel({ log, compact = false }: { log: RequestLogView; compact?: boolean }) {
  const { t } = useI18n();
  const effort = log.reasoning_effort
    ? reasoningEffortLabel(log.reasoning_effort)
    : null;

  const sameModel = log.client_model === log.upstream_model;

  return (
    <span className="flex min-w-0 flex-col items-start gap-0.5 wrap-anywhere">
      <span className={cn(compact && "line-clamp-2")}>
        <span className="text-xs text-muted-foreground">{sameModel ? t("Requested & upstream") : t("Requested model")}: </span>
        <span className="font-medium">{log.client_model}</span>
      </span>
      {!sameModel && (
        <span className={cn("text-xs", compact && "line-clamp-2")}>
          <span className="text-muted-foreground">{t("Upstream model")}: </span>
          <span>{log.upstream_model ?? t("Not recorded")}</span>
        </span>
      )}
      {effort || log.fast_mode ? (
        <span className="flex max-w-full flex-wrap gap-1.5">
          {effort && (
            <Badge variant="warning" className="max-w-full" aria-label={`${t("Reasoning effort")}: ${effort}`} title={`${t("Reasoning effort")}: ${effort}`}>
              <span className="truncate">{effort}</span>
            </Badge>
          )}
          {log.fast_mode && (
            <Badge variant="success" aria-label={t("Fast mode")} title={t("Fast mode")}>
              {t("Fast")}
            </Badge>
          )}
        </span>
      ) : null}
    </span>
  );
}

export function formatOutputTps(value: string | null): string {
  const formatted = formatTokens(value == null ? null : Number(value));
  return formatted === "—" ? formatted : `${formatted} tok/s`;
}

export function RequestDuration({ log }: { log: RequestLogView }) {
  const { t } = useI18n();
  const ttft = formatDurationMs(log.ttft_ms);
  const total = formatDurationMs(log.total_duration_ms);
  const tps = formatOutputTps(log.output_tokens_per_second);
  const ttftLabel = `${t("TTFT")}: ${ttft}`;
  const totalLabel = `${t("Total duration")}: ${total}`;
  const tpsLabel = `${t("TPS")}: ${tps}`;

  return (
    <span className="flex flex-col gap-0.5 wrap-anywhere tabular-nums">
      <span
        className="text-xs text-muted-foreground"
        aria-label={ttftLabel}
        title={ttftLabel}
      >
        {ttft}
      </span>
      <span aria-label={totalLabel} title={totalLabel}>
        {total}
      </span>
      <span
        className="text-xs text-muted-foreground"
        aria-label={tpsLabel}
        title={tpsLabel}
      >
        {tps}
      </span>
    </span>
  );
}

export function RequestCost({ log }: { log: RequestLogView }) {
  const { t } = useI18n();

  return (
    <span className="flex flex-wrap items-center gap-1.5 wrap-anywhere tabular-nums">
      <span>{formatUsd(log.cost_amount)}</span>
      {log.peak_pricing ? (
        <Badge
          variant="warning"
          aria-label={t("Peak pricing")}
          title={t("Peak pricing")}
        >
          {t("Peak")}
        </Badge>
      ) : null}
    </span>
  );
}


export function Inspector({ mobile, onClose, closeRef, controls, children }: {
  mobile: boolean; onClose: () => void; closeRef: RefObject<HTMLButtonElement | null>;
  controls: ReactNode; children: ReactNode;
}) {
  const { t } = useI18n();
  if (mobile) return (
    <Sheet open onOpenChange={open => { if (!open) onClose(); }}>
      <SheetContent className="overflow-y-auto data-[side=right]:w-full">
        <SheetHeader className="border-b pr-12"><SheetTitle>{t("Request log")}</SheetTitle>
          {controls}</SheetHeader>
        {children}
      </SheetContent>
    </Sheet>
  );
  return (
    <aside aria-label={t("Request log")} className="order-first min-w-0 self-start border-b bg-background @min-[44rem]:sticky @min-[44rem]:top-3 @min-[44rem]:order-last @min-[44rem]:border-b-0 @min-[44rem]:border-l"
      onKeyDown={e => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
      <header className="flex items-center gap-2 border-b bg-muted/30 px-3 py-2">
        <PanelRight className="size-4 text-primary" /><h2 className="mr-auto text-sm font-medium">{t("Request inspector")}</h2>
        {controls}<Button ref={closeRef} variant="ghost" size="icon-sm" aria-label={t("Close inspector")} onClick={onClose}><X /></Button>
      </header>
      <div className="max-h-[55dvh] overflow-y-auto @min-[44rem]:max-h-[calc(100dvh-6rem)]">{children}</div>
    </aside>
  );
}

