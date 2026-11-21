import { type ReactNode, type RefObject, useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, ChevronLeft, ChevronRight, Columns3, ListFilter, PanelRight, Search, X } from "lucide-react";
import { useSearchParams } from "react-router";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuGroup, DropdownMenuCheckboxItem } from "@/components/ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Badge } from "@/components/ui/badge";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useRequestLog } from "@/features/request-logs/api";
import type {
  ApiKeyView,
  ControlPlaneUser,
  ListQuery,
  RequestLogView,
} from "@/api/types";
import { dateTimeLocalToIso, formatDateTime, formatDateTimeLocalInput, formatRelative } from "@/lib/dates";
import { formatDecimal, formatDurationMs, formatTokens, formatUsd } from "@/lib/formatters";
import { API_FORMATS, API_OPERATIONS, apiFormatLabel, outcomeLabel, outcomeVariant } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";
import { useIsMobile } from "@/hooks/use-mobile";

const LIMITS = [25, 50, 100] as const;
const OUTCOMES = ["succeeded", "failed", "rejected", "cancelled"] as const;
const uuid = /^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i;

type ApiFormatFilter = NonNullable<ListQuery["api_format"]>;
type ApiOperationFilter = NonNullable<ListQuery["api_operation"]>;
type OutcomeFilter = NonNullable<ListQuery["outcome"]>;

interface RequestLogListResult {
  data: RequestLogView[] | undefined;
  isLoading: boolean;
  isFetching: boolean;
  error: unknown;
  refetch: () => Promise<unknown>;
}

type UseRequestLogs = (filters: ListQuery) => RequestLogListResult;

interface RequestLogApiKeyOption extends Pick<ApiKeyView, "id" | "name"> {
  user_id?: string;
}

interface RequestLogFilterDraft {
  limit: (typeof LIMITS)[number];
  user_id: string;
  api_key_id: string;
  model: string;
  api_format: "" | ApiFormatFilter;
  api_operation: "" | ApiOperationFilter;
  outcome: "" | OutcomeFilter;
  started_after: string;
  started_before: string;
  billed: "" | "true" | "false";
}

const emptyFilters: RequestLogFilterDraft = {
  limit: 50,
  user_id: "",
  api_key_id: "",
  model: "",
  api_format: "",
  api_operation: "",
  outcome: "",
  started_after: "",
  started_before: "",
  billed: "",
};

const REQUEST_LOG_FILTER_KEYS = [
  "limit",
  "user_id",
  "api_key_id",
  "model",
  "api_format",
  "api_operation",
  "outcome",
  "started_after",
  "started_before",
  "billed",
] as const satisfies readonly (keyof ListQuery)[];

function readFilters(search: string, system: boolean): ListQuery {
  const params = new URLSearchParams(search);
  const identifier = (key: string) => {
    const value = params.get(key);
    return value && uuid.test(value) ? value : undefined;
  };
  const instant = (key: string) => {
    const value = params.get(key);
    if (!value || !/(?:Z|[+-]\d\d:\d\d)$/i.test(value) || !Number.isFinite(Date.parse(value))) return undefined;
    return value;
  };
  return {
    limit: LIMITS.find(v => String(v) === params.get("limit")) ?? 50,
    user_id: system ? identifier("user_id") : undefined, api_key_id: identifier("api_key_id"),
    model: params.get("model")?.trim() || undefined,
    api_format: API_FORMATS.find(v => v === params.get("api_format")),
    api_operation: API_OPERATIONS.find(v => v === params.get("api_operation")),
    outcome: OUTCOMES.find(v => v === params.get("outcome")),
    started_after: instant("started_after"), started_before: instant("started_before"),
    billed: params.get("billed") === "true" ? true : params.get("billed") === "false" ? false : undefined,
  };
}

function filterDraft(query: ListQuery): RequestLogFilterDraft {
  return {
    limit: LIMITS.find(value => value === query.limit) ?? 50,
    user_id: query.user_id ?? "",
    api_key_id: query.api_key_id ?? "",
    model: query.model ?? "",
    api_format: query.api_format ?? "",
    api_operation: query.api_operation ?? "",
    outcome: query.outcome ?? "",
    started_after: formatDateTimeLocalInput(query.started_after, true),
    started_before: formatDateTimeLocalInput(query.started_before, true),
    billed: query.billed === undefined ? "" : query.billed ? "true" : "false",
  };
}

interface RequestLogsViewProps {
  title: string;
  basePath: string;
  useLogs: UseRequestLogs;
  scope: "own" | "system";
  users?: ControlPlaneUser[];
  apiKeys?: RequestLogApiKeyOption[];
  modelOptions?: string[];
}

function optionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed || undefined;
}

function tokenRemainder(
  total: number | null | undefined,
  subset: number | null | undefined,
): number | null {
  if (total === null || total === undefined) return null;
  return Math.max(0, total - (subset ?? 0));
}

function requestProtocolLabel(
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

function requestOperationLabel(
  operation: RequestLogView["api_operation"],
  t: (value: string) => string,
) {
  switch (operation) {
    case "chat_completion":
      return t("Chat Completions");
    case "responses":
      return t("Responses");
    case "responses-ws":
      return "Responses WebSocket";
    case "web_search":
      return t("Standalone web search");
    case "images_generation":
      return t("Image generation");
    case "images_edit":
      return t("Image edit");
  }
}

function TruncatedName({
  value,
  className,
}: {
  value: string | null;
  className: string;
}) {
  if (!value) return "—";
  return (
    <Tooltip disableHoverablePopup>
      <TooltipTrigger
        render={
          <span
            tabIndex={0}
            className={cn(
              "block truncate outline-none focus-visible:ring-2 focus-visible:ring-ring",
              className,
            )}
          />
        }
      >
        {value}
      </TooltipTrigger>
      <TooltipContent className="break-words">{value}</TooltipContent>
    </Tooltip>
  );
}

function TokenUsage({ log }: { log: RequestLogView }) {
  const { t } = useI18n();
  const uncachedInput = tokenRemainder(log.input_tokens, log.cached_input_tokens);
  const uncachedInputLabel = `${t("Uncached input")}: ${formatTokens(uncachedInput)}`;
  const cachedInputLabel = `${t("Cached input")}: ${formatTokens(log.cached_input_tokens)}`;
  const outputLabel = `${t("Output tokens")}: ${formatTokens(log.output_tokens)}`;
  const reasoningLabel = `${t("Reasoning tokens")}: ${formatTokens(log.reasoning_tokens)}`;

  return (
    <span className="flex flex-col gap-1.5 tabular-nums">
      <span className="flex items-center gap-2">
        <span
          className="flex min-w-20 items-center gap-1.5"
          aria-label={uncachedInputLabel}
          title={uncachedInputLabel}
        >
          <ArrowUp className="size-4 shrink-0 text-success" aria-hidden />
          <span>{formatTokens(uncachedInput)}</span>
        </span>
        {log.cached_input_tokens !== null && log.cached_input_tokens > 0 ? (
          <Badge variant="success" aria-label={cachedInputLabel} title={cachedInputLabel}>
            {formatTokens(log.cached_input_tokens)}
          </Badge>
        ) : null}
      </span>
      <span className="flex items-center gap-2">
        <span
          className="flex min-w-20 items-center gap-1.5"
          aria-label={outputLabel}
          title={outputLabel}
        >
          <ArrowDown className="size-4 shrink-0 text-info" aria-hidden />
          <span>{formatTokens(log.output_tokens)}</span>
        </span>
        {log.reasoning_tokens !== null && log.reasoning_tokens > 0 ? (
          <Badge variant="info" aria-label={reasoningLabel} title={reasoningLabel}>
            {formatTokens(log.reasoning_tokens)}
          </Badge>
        ) : null}
      </span>
    </span>
  );
}

function reasoningEffortLabel(value: string): string {
  if (value === "xhigh") return "XHigh";
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}

function RequestModel({ log, nameClassName }: { log: RequestLogView; nameClassName?: string }) {
  const { t } = useI18n();
  const effort = log.reasoning_effort
    ? reasoningEffortLabel(log.reasoning_effort)
    : null;

  return (
    <span className="flex flex-wrap items-center gap-1.5">
      <span className={cn("font-medium wrap-anywhere", nameClassName)}>{log.client_model}</span>
      {effort ? (
        <Badge
          variant="warning"
          aria-label={`${t("Reasoning effort")}: ${effort}`}
          title={`${t("Reasoning effort")}: ${effort}`}
        >
          {effort}
        </Badge>
      ) : null}
      {log.fast_mode ? (
        <Badge variant="success" aria-label={t("Fast mode")} title={t("Fast mode")}>
          Fast
        </Badge>
      ) : null}
    </span>
  );
}

function formatOutputTps(value: string | null): string {
  const formatted = formatDecimal(value, 1);
  return formatted === "—" ? formatted : `${formatted} tok/s`;
}

function RequestDuration({ log }: { log: RequestLogView }) {
  const { t } = useI18n();
  const ttft = formatDurationMs(log.ttft_ms);
  const total = formatDurationMs(log.total_duration_ms);
  const tps = formatOutputTps(log.output_tokens_per_second);
  const ttftLabel = `${t("TTFT")}: ${ttft}`;
  const totalLabel = `${t("Total duration")}: ${total}`;
  const tpsLabel = `${t("TPS")}: ${tps}`;

  return (
    <span className="flex flex-col gap-0.5 whitespace-nowrap tabular-nums">
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

function RequestCost({ log }: { log: RequestLogView }) {
  const { t } = useI18n();

  return (
    <span className="flex flex-wrap items-center gap-1.5 tabular-nums">
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

function toQuery(draft: RequestLogFilterDraft): ListQuery {
  return {
    limit: draft.limit,
    user_id: optionalText(draft.user_id),
    api_key_id: optionalText(draft.api_key_id),
    model: optionalText(draft.model),
    api_format: draft.api_format || undefined,
    api_operation: draft.api_operation || undefined,
    outcome: draft.outcome || undefined,
    started_after: dateTimeLocalToIso(draft.started_after) ?? undefined,
    started_before: dateTimeLocalToIso(draft.started_before) ?? undefined,
    billed: draft.billed === "" ? undefined : draft.billed === "true",
  };
}

function Inspector({ mobile, onClose, closeRef, controls, children }: {
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
    <aside aria-label={t("Request log")} className="sticky top-3 min-w-0 self-start border-l bg-background"
      onKeyDown={e => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
      <header className="flex items-center gap-2 border-b bg-muted/30 px-3 py-2">
        <PanelRight className="size-4 text-primary" /><h2 className="mr-auto text-sm font-medium">{t("Request inspector")}</h2>
        {controls}<Button ref={closeRef} variant="ghost" size="icon-sm" aria-label={t("Close inspector")} onClick={onClose}><X /></Button>
      </header>
      <div className="max-h-[calc(100dvh-6rem)] overflow-y-auto">{children}</div>
    </aside>
  );
}

export function RequestLogsView({
  title,
  basePath,
  useLogs,
  scope,
  users = [],
  apiKeys = [],
  modelOptions = [],
}: RequestLogsViewProps) {
  const { t } = useI18n();
  const isSystemView = scope === "system";
  const isMobile = useIsMobile();
  const [filtersOpen, setFiltersOpen] = useState(false);
  const filterToggle = useRef<HTMLButtonElement>(null);
  const [params, setParams] = useSearchParams();
  const filterParams = new URLSearchParams(params);
  filterParams.delete("id");
  const search = filterParams.toString();
  const filters = useMemo(() => readFilters(search, isSystemView), [search, isSystemView]);
  const [draft, setDraft] = useState(() => filterDraft(filters));
  useEffect(() => { setDraft(filterDraft(filters)); }, [filters]);
  const query = useLogs(filters);
  const requestedId = params.get("id");
  const selectedId = requestedId && uuid.test(requestedId) ? requestedId : null;
  const [idSearch, setIdSearch] = useState("");
  const [hiddenColumns, setHiddenColumns] = useState<Set<string>>(() => new Set());
  const returnFocus = useRef<HTMLElement | null>(null);
  const inspectorClose = useRef<HTMLButtonElement>(null);
  const setSelectedId = (id: string | null) => {
    if (!selectedId && id) returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const next = new URLSearchParams(params);
    if (id) next.set("id", id); else next.delete("id");
    setParams(next, { replace: true, preventScrollReset: true });
    if (!id) requestAnimationFrame(() => {
      const opener = returnFocus.current;
      (opener?.isConnected && opener !== document.body ? opener : filterToggle.current)?.focus();
    });
  };
  const inspectorOpen = Boolean(selectedId);
  useEffect(() => { if (inspectorOpen && !isMobile) inspectorClose.current?.focus({ preventScroll: true }); }, [inspectorOpen, isMobile]);
  const prefix = idSearch.trim().toLowerCase();
  const matchingRows = (query.data ?? []).filter(row => row.id.toLowerCase().startsWith(prefix));
  const index = matchingRows.findIndex(row => row.id === selectedId);
  const [discoveredModels, setDiscoveredModels] = useState<string[]>([]);
  const detail = useRequestLog(basePath, selectedId);

  useEffect(() => {
    const loggedModels =
      query.data?.flatMap((log) => [log.client_model, log.upstream_model ?? ""]) ?? [];
    const nextModels = loggedModels.filter(Boolean);
    if (nextModels.length === 0) return;

    setDiscoveredModels((current) => {
      const merged = [...new Set([...current, ...nextModels])].sort((left, right) =>
        left.localeCompare(right),
      );
      return merged.length === current.length &&
        merged.every((model, index) => model === current[index])
        ? current
        : merged;
    });
  }, [query.data]);

  const availableApiKeys = useMemo(
    () => apiKeys.filter((key) => !draft.user_id || key.user_id === draft.user_id),
    [apiKeys, draft.user_id],
  );
  const availableModels = useMemo(
    () =>
      [...new Set([...modelOptions, ...discoveredModels, draft.model])]
        .filter(Boolean)
        .sort((left, right) => left.localeCompare(right)),
    [discoveredModels, draft.model, modelOptions],
  );
  const userNamesById = useMemo(
    () => new Map(users.map((user) => [user.id, user.display_name])),
    [users],
  );
  const apiKeyLabel = (key: RequestLogApiKeyOption) => {
    const ownerName = key.user_id ? userNamesById.get(key.user_id) : undefined;
    return isSystemView && ownerName ? `${key.name} · ${ownerName}` : key.name;
  };
  const appliedApiKey = apiKeys.find((key) => key.id === filters.api_key_id);
  const filterLabels: [string, string | undefined][] = [
    ["User", isSystemView && filters.user_id ? (userNamesById.get(filters.user_id) ?? t("Unavailable")) : undefined],
    ["API key", appliedApiKey ? apiKeyLabel(appliedApiKey) : filters.api_key_id && t("Unavailable")],
    ["Model", filters.model],
    ["API format", filters.api_format && apiFormatLabel(filters.api_format)],
    ["Operation", filters.api_operation && requestOperationLabel(filters.api_operation, t)],
    ["Outcome", filters.outcome && outcomeLabel(filters.outcome)],
    ["Billing", filters.billed === undefined ? undefined : t(filters.billed ? "Billed" : "Unbilled")],
    ["From", filters.started_after && formatDateTime(filters.started_after)],
    ["To", filters.started_before && formatDateTime(filters.started_before)],
    ["Results", filters.limit !== undefined && filters.limit !== emptyFilters.limit
      ? t("Last {count}", { count: filters.limit })
      : undefined],
  ];
  const appliedConditions = filterLabels.flatMap(([label, value]) => value ? [{ label, value }] : []);

  const updateDraft = (partial: Partial<RequestLogFilterDraft>) => {
    setDraft((previous) => ({ ...previous, ...partial }));
  };
  const runQuery = (next: ListQuery) => {
    if (REQUEST_LOG_FILTER_KEYS.every(key => filters[key] === next[key])) void query.refetch();
    const nextParams = new URLSearchParams();
    for (const key of REQUEST_LOG_FILTER_KEYS) {
      const value = next[key];
      if (value !== undefined && value !== null && !(key === "limit" && value === 50)) nextParams.set(key, String(value));
    }
    setParams(nextParams, { replace: true, preventScrollReset: true });
  };
  const closeFilters = () => {
    setFiltersOpen(false);
    filterToggle.current?.focus();
  };
  const applyFilters = () => {
    const next = toQuery(draft);
    for (const k of ["started_after", "started_before"] as const) {
      if (draft[k] === formatDateTimeLocalInput(filters[k], true)) next[k] = filters[k];
    }
    runQuery(next);
    closeFilters();
  };
  const clearFilters = () => {
    setIdSearch("");
    setDraft(emptyFilters);
    runQuery(toQuery(emptyFilters));
    closeFilters();
  };

  const columns: Column<RequestLogView>[] = [
    {
      key: "started",
      header: t("Started"),
      render: (log) => (
        <span className="flex flex-col">
          <span>{formatDateTime(log.started_at)}</span>
          <span className="text-xs text-muted-foreground">{formatRelative(log.started_at)}</span>
        </span>
      ),
    },
    {
      key: "model",
      header: t("Model"),
      className: "min-w-36 max-w-56 whitespace-normal",
      render: (log) => (
        <div aria-current={selectedId === log.id || undefined}>
          <RequestModel log={log} nameClassName="line-clamp-2" />
        </div>
      ),
    },
    {
      key: "operation-protocol",
      header: t("Operation / protocol"),
      render: (log) => (
        <span className="flex flex-col items-start gap-1">
          <span className="font-medium">
            {requestOperationLabel(log.api_operation, t)}
          </span>
          <Badge variant="outline">
            {requestProtocolLabel(log.request_protocol, t)}
          </Badge>
        </span>
      ),
    },
    {
      key: "channel-group",
      header: t("Channel group"),
      className: "w-44 max-w-44",
      render: (log) => (
        <TruncatedName value={log.channel_group_name} className="max-w-44" />
      ),
    },
    ...(isSystemView
      ? [
          {
            key: "channel",
            header: t("Channel"),
            className: "w-36 max-w-36",
            render: (log: RequestLogView) => (
              <TruncatedName value={log.channel_name} className="max-w-36" />
            ),
          },
          {
            key: "user",
            header: t("User"),
            className: "w-36 max-w-36",
            render: (log: RequestLogView) => (
              <TruncatedName value={log.user_name} className="max-w-36" />
            ),
          },
        ]
      : []),
    {
      key: "outcome",
      header: t("Outcome"),
      render: (log) => (
        <Badge variant={outcomeVariant(log.outcome)}>{outcomeLabel(log.outcome)}</Badge>
      ),
    },
    {
      key: "tokens",
      header: t("Tokens"),
      className: "min-w-44",
      render: (log) => <TokenUsage log={log} />,
    },
    {
      key: "cost",
      header: t("Cost"),
      render: (log) => <RequestCost log={log} />,
    },
    {
      key: "duration",
      header: t("Duration"),
      render: (log) => <RequestDuration log={log} />,
    },
  ];
  const compactColumns: Column<RequestLogView>[] = [
    {
      key: "request",
      header: t("Request log"),
      className: "w-full whitespace-normal",
      render: (log) => (
        <div className="flex flex-col items-start gap-1">
          <time dateTime={log.started_at} className="text-xs text-muted-foreground">
            {formatDateTime(log.started_at)}
          </time>
          <span className="line-clamp-2 font-medium wrap-anywhere">{log.client_model}</span>
          <Badge variant={outcomeVariant(log.outcome)}>{outcomeLabel(log.outcome)}</Badge>
        </div>
      ),
    },
  ];

  return (
    <Collapsible
      className="flex min-w-0 flex-col gap-3"
      open={filtersOpen}
      onOpenChange={setFiltersOpen}
    >
      <PageHeader title={title} />
      <div className="flex min-w-0 items-center gap-1 overflow-x-auto border-b" role="group" aria-label={t("Outcome")}>
        {(["", "succeeded", "failed", "rejected", "cancelled"] as const).map(value => (
          <button key={value} type="button" aria-pressed={(filters.outcome ?? "") === value}
            className="shrink-0 border-b-2 border-transparent px-2 py-2 text-xs font-medium text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring aria-pressed:border-primary aria-pressed:text-foreground"
            onClick={() => runQuery({ ...filters, outcome: value || undefined })}>
            {value ? outcomeLabel(value) : t("All")}
          </button>
        ))}
      </div>
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex items-center gap-2 border-b pb-2">
          <form className="mr-auto flex min-w-0 max-w-72 flex-1 items-center gap-1" onSubmit={e => {
            e.preventDefault(); const id = idSearch.trim(); if (uuid.test(id)) setSelectedId(id);
          }}>
            <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden />
            <Input aria-label={t("Find request ID")} placeholder={t("Find request ID")} value={idSearch} type="search" autoComplete="off" spellCheck={false}
              className="h-8 border-0 bg-transparent px-1 shadow-none" onChange={e => setIdSearch(e.target.value)} />
            {uuid.test(idSearch.trim()) ? <Button type="submit" size="sm" variant="ghost">{t("View details")}</Button> : null}
          </form>
          <CollapsibleTrigger
            ref={filterToggle}
            render={<Button variant="outline" className="self-start scroll-mt-20" />}
          >
            {t("Filters ({count})", { count: appliedConditions.length })}
            <ListFilter data-icon="inline-end" />
          </CollapsibleTrigger>
          {!isMobile && <DropdownMenu><DropdownMenuTrigger render={<Button variant="outline" size="sm" />}><Columns3 />{t("Display columns")}</DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-44"><DropdownMenuGroup>{columns.map(col => (
              <DropdownMenuCheckboxItem key={col.key} checked={!hiddenColumns.has(col.key)} disabled={col.key === "model"} closeOnClick={false}
                onCheckedChange={checked => setHiddenColumns(prev => { const next = new Set(prev); if (checked) next.delete(col.key); else next.add(col.key); return next; })}>{col.header}</DropdownMenuCheckboxItem>
            ))}</DropdownMenuGroup></DropdownMenuContent></DropdownMenu>}
          </div>
          {appliedConditions.length > 0 ? (
            <div className="flex flex-wrap items-start gap-2">
              <ul aria-label={t("Applied filters")} className="flex min-w-0 flex-wrap gap-2">
                {appliedConditions.map(({ label, value }) => (
                  <li key={label} className="min-w-0 max-w-full">
                    <Badge variant="secondary" className="block h-auto whitespace-normal wrap-anywhere">
                      {t(label)}: {value}
                    </Badge>
                  </li>
                ))}
              </ul>
              <Button type="button" variant="ghost" size="sm" onClick={clearFilters}>
                <X data-icon="inline-start" />
                {t("Clear filters")}
              </Button>
            </div>
          ) : null}
        </div>
      <CollapsibleContent className="border-b pb-3">
          <form
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              applyFilters();
            }}
          >
            <FieldGroup className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-7">
              {isSystemView ? (
                <Field>
                  <FieldLabel htmlFor="request-log-user">{t("User")}</FieldLabel>
                  <Select
                    value={draft.user_id || "__all__"}
                    onValueChange={(value) => {
                      const userId = value === "__all__" ? "" : value;
                      const selectedKey = apiKeys.find(
                        (key) => key.id === draft.api_key_id,
                      );
                      updateDraft({
                        user_id: userId,
                        api_key_id:
                          draft.api_key_id &&
                          (!userId || selectedKey?.user_id === userId)
                            ? draft.api_key_id
                            : "",
                      });
                    }}
                  >
                    <SelectTrigger id="request-log-user" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                      <SelectItem value="__all__">{t("All users")}</SelectItem>
                      {draft.user_id && !userNamesById.has(draft.user_id) && <SelectItem value={draft.user_id} disabled>{t("Unavailable")}</SelectItem>}
                        {users.map((user) => (
                          <SelectItem key={user.id} value={user.id}>
                            {user.display_name}
                            {user.email ? ` · ${user.email}` : ""}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
              ) : null}
              <Field>
                <FieldLabel htmlFor="request-log-api-key">{t("API key")}</FieldLabel>
                <Select
                  value={draft.api_key_id || "__all__"}
                  onValueChange={(value) =>
                    updateDraft({
                      api_key_id: value === "__all__" ? "" : value,
                    })
                  }
                >
                  <SelectTrigger id="request-log-api-key" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                    <SelectItem value="__all__">{t("All API keys")}</SelectItem>
                    {draft.api_key_id && !availableApiKeys.some(key => key.id === draft.api_key_id) && <SelectItem value={draft.api_key_id} disabled>{t("Unavailable")}</SelectItem>}
                      {availableApiKeys.map((key) => (
                        <SelectItem key={key.id} value={key.id}>
                          {apiKeyLabel(key)}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field className={cn(!isSystemView && "xl:col-span-2")}>
                <FieldLabel htmlFor="request-log-model">{t("Model")}</FieldLabel>
                <Select
                  value={draft.model || "__all__"}
                  onValueChange={(value) =>
                    updateDraft({ model: value === "__all__" ? "" : value })
                  }
                >
                  <SelectTrigger id="request-log-model" aria-describedby="request-log-model-help" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">{t("All models")}</SelectItem>
                      {availableModels.map((model) => (
                        <SelectItem key={model} value={model}>
                          {model}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor="request-log-format">{t("API format")}</FieldLabel>
                <Select
                  value={draft.api_format || "__all__"}
                  onValueChange={(value) =>
                    updateDraft({
                      api_format:
                        value === "__all__" ? "" : (value as ApiFormatFilter),
                    })
                  }
                >
                  <SelectTrigger id="request-log-format" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">{t("All formats")}</SelectItem>
                      {API_FORMATS.map((format) => (
                        <SelectItem key={format} value={format}>
                          {apiFormatLabel(format)}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor="request-log-operation">{t("Operation")}</FieldLabel>
                <Select
                  value={draft.api_operation || "__all__"}
                  onValueChange={(value) =>
                    updateDraft({
                      api_operation:
                        value === "__all__"
                          ? ""
                          : (value as ApiOperationFilter),
                    })
                  }
                >
                  <SelectTrigger id="request-log-operation" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">{t("All operations")}</SelectItem>
                      {API_OPERATIONS.map((operation) => (
                        <SelectItem key={operation} value={operation}>
                          {requestOperationLabel(operation, t)}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor="request-log-outcome">{t("Outcome")}</FieldLabel>
                <Select
                  value={draft.outcome || "__all__"}
                  onValueChange={(value) =>
                    updateDraft({
                      outcome: value === "__all__" ? "" : (value as OutcomeFilter),
                    })
                  }
                >
                  <SelectTrigger id="request-log-outcome" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">{t("All outcomes")}</SelectItem>
                      {OUTCOMES.map((outcome) => (
                        <SelectItem key={outcome} value={outcome}>
                          {outcomeLabel(outcome)}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor="request-log-billing">{t("Billing")}</FieldLabel>
                <Select
                  value={draft.billed || "__all__"}
                  onValueChange={(value) =>
                    updateDraft({
                      billed:
                        value === "__all__" ? "" : (value as RequestLogFilterDraft["billed"]),
                    })
                  }
                >
                  <SelectTrigger id="request-log-billing" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="__all__">{t("All billing")}</SelectItem>
                      <SelectItem value="true">{t("Billed")}</SelectItem>
                      <SelectItem value="false">{t("Unbilled")}</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field className="xl:col-span-2">
                <FieldLabel htmlFor="request-log-started-after">{t("From")}</FieldLabel>
                <Input
                  id="request-log-started-after"
                  type="datetime-local"
                  step="1"
                  value={draft.started_after}
                  onChange={(event) => updateDraft({ started_after: event.target.value })}
                />
              </Field>
              <Field className="xl:col-span-2">
                <FieldLabel htmlFor="request-log-started-before">{t("To")}</FieldLabel>
                <Input
                  id="request-log-started-before"
                  type="datetime-local"
                  step="1"
                  value={draft.started_before}
                  onChange={(event) => updateDraft({ started_before: event.target.value })}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="request-log-limit">{t("Results")}</FieldLabel>
                <Select
                  value={String(draft.limit)}
                  onValueChange={(value) =>
                    updateDraft({ limit: Number(value) as RequestLogFilterDraft["limit"] })
                  }
                >
                  <SelectTrigger id="request-log-limit" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {LIMITS.map((value) => (
                        <SelectItem key={value} value={String(value)}>
                          {t("Last {count}", { count: value })}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field className="justify-end">
                <FieldLabel className="sr-only">{t("Filter actions")}</FieldLabel>
                <div className="flex gap-2">
                  <Button
                    type="submit"
                    className="flex-1"
                    disabled={query.isFetching}
                  >
                    {query.isFetching ? (
                      <Spinner data-icon="inline-start" />
                    ) : (
                      <Search data-icon="inline-start" />
                    )}
                    {t("Apply")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    className="flex-1"
                    onClick={clearFilters}
                  >
                    <X data-icon="inline-start" />
                    {t("Clear")}
                  </Button>
                </div>
              </Field>
            </FieldGroup>
            <p id="request-log-model-help" className="text-xs text-muted-foreground">{t("Model matches either the client model or the upstream model exactly.")}</p>
          </form>
      </CollapsibleContent>
      <div className={cn("grid min-w-0 items-start", selectedId && !isMobile && "grid-cols-[minmax(0,1fr)_minmax(16rem,31%)]")}>
        <section aria-label={t("Requests")} className="min-w-0 [&_tr:has([aria-current=true])]:bg-accent/60">
          <AsyncResource
            isLoading={query.isLoading}
            error={query.error}
            isEmpty={matchingRows.length === 0}
            emptyTitle={t("No request logs")}
            emptyDescription={t("There are no logged requests matching these filters.")}
          >
            <ResourceTable
              columns={isMobile ? compactColumns : columns.filter(col => !hiddenColumns.has(col.key))}
              rows={matchingRows}
              rowKey={(log) => log.id}
              onRowClick={(log) => setSelectedId(log.id)}
              rowActionLabel={t("View details")}
            />
          </AsyncResource>
          <p className="mt-3 text-xs text-muted-foreground">{t("The gateway does not store request bodies or successful response bodies. Bounded upstream error details may be retained.")}</p>
        </section>
      {selectedId && <Inspector mobile={isMobile} closeRef={inspectorClose} onClose={() => setSelectedId(null)} controls={<div className="flex items-center gap-0.5">
        <Button size="icon-sm" variant="ghost" aria-label={t("Previous request")} disabled={index < 1} onClick={() => setSelectedId(matchingRows[index-1].id)}><ChevronLeft /></Button>
        <Button size="icon-sm" variant="ghost" aria-label={t("Next request")} disabled={index < 0 || index + 1 >= matchingRows.length} onClick={() => setSelectedId(matchingRows[index+1].id)}><ChevronRight /></Button>
      </div>}>
        <p className="px-4 pt-3 font-mono text-xs break-all text-muted-foreground select-all" aria-label={t("Request ID")}>{selectedId}</p>
        <AsyncResource isLoading={detail.isLoading} error={detail.error}>
          {detail.data ? (
            <dl className="grid grid-cols-2 gap-x-3 gap-y-4 p-4 [&>div]:min-w-0 [&>div:has(.whitespace-pre-wrap)]:col-span-2">
              <DetailField label={t("Started")} value={formatDateTime(detail.data.started_at)} />
              <DetailField label={t("Model")} value={<RequestModel log={detail.data} />} />
              <DetailField
                label={t("Operation")}
                value={requestOperationLabel(detail.data.api_operation, t)}
              />
              <DetailField
                label={t("Protocol")}
                value={requestProtocolLabel(detail.data.request_protocol, t)}
              />
              <DetailField
                label={t("Channel group")}
                value={detail.data.channel_group_name ?? "—"}
              />
              {isSystemView ? (
                <>
                  <DetailField label={t("Channel")} value={detail.data.channel_name ?? "—"} />
                  <DetailField label={t("User")} value={detail.data.user_name ?? "—"} />
                </>
              ) : null}
              <DetailField
                label={t("Outcome")}
                value={
                  <Badge variant={outcomeVariant(detail.data.outcome)}>
                    {outcomeLabel(detail.data.outcome)}
                  </Badge>
                }
              />
              <DetailField label={t("Tokens")} value={<TokenUsage log={detail.data} />} />
              <DetailField label={t("Cost")} value={<RequestCost log={detail.data} />} />
              <DetailField
                label={t("Duration")}
                value={<RequestDuration log={detail.data} />}
              />
              <DetailField label={t("HTTP")} value={detail.data.response_status_code ?? "—"} />
              <DetailField label={t("Error code")} value={detail.data.error_code ?? "—"} mono />
              <DetailField
                label={t("Error detail")}
                value={
                  <span className="whitespace-pre-wrap break-words">
                    {detail.data.error_summary ?? "—"}
                  </span>
                }
              />
              <DetailField label={t("Completed")} value={formatDateTime(detail.data.completed_at)} />
            </dl>
          ) : null}
        </AsyncResource>
      </Inspector>}
      </div>
    </Collapsible>
  );
}
