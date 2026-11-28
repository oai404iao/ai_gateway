import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Columns3, ListFilter, Search, X } from "lucide-react";
import { useSearchParams } from "react-router";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Combobox, ComboboxContent, ComboboxEmpty, ComboboxInput, ComboboxItem, ComboboxList } from "@/components/ui/combobox";
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
import { Badge } from "@/components/ui/badge";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { CursorPagination } from "@/components/shared/cursor-pagination";
import { DetailField } from "@/components/shared/detail-field";
import {
  ApiKeyFilterCombobox,
  UserFilterCombobox,
} from "@/components/shared/entity-filter-comboboxes";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useAdminApiKey, useUser } from "@/features/admin/api";
import { useOwnApiKey } from "@/features/api-keys/api";
import { useModelOptions, useRequestLog } from "@/features/request-logs/api";
import type {
  AdminRequestLogQuery,
  RequestLogView,
} from "@/api/types";
import { formatDateTime, formatDateTimeLocalInput, formatRelative } from "@/lib/dates";
import { API_FORMATS, API_OPERATIONS, apiFormatLabel, apiOperationLabel, outcomeLabel, outcomeVariant } from "@/lib/permissions";
import { useCursorPagination } from "@/lib/use-cursor-pagination";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";
import { useIsMobile } from "@/hooks/use-mobile";
import {
  emptyFilters,
  filterDraft,
  LIMITS,
  OUTCOMES,
  readFilters,
  REQUEST_LOG_FILTER_KEYS,
  toQuery,
  uuid,
  type ApiFormatFilter,
  type ApiOperationFilter,
  type OutcomeFilter,
  type RequestLogFilterDraft,
  type RequestLogsViewProps,
} from "./request-logs-filters";
import {
  Inspector,
  RequestCost,
  RequestDuration,
  RequestModel,
  requestProtocolLabel,
  TokenUsage,
  TruncatedName,
} from "./request-logs-cells";
export function RequestLogsView({
  title,
  basePath,
  useLogs,
  scope,
}: RequestLogsViewProps) {
  const { t } = useI18n();
  const isSystemView = scope === "system";
  const isMobile = useIsMobile();
  const [filtersOpen, setFiltersOpen] = useState(false);
  const modelOptions = useModelOptions(filtersOpen);
  const filterToggle = useRef<HTMLButtonElement>(null);
  const [params, setParams] = useSearchParams();
  const filterParams = new URLSearchParams(params);
  filterParams.delete("id");
  const search = filterParams.toString();
  const filters = useMemo(() => readFilters(search, isSystemView), [search, isSystemView]);
  const cursorPagination = useCursorPagination();
  const resetCursor = cursorPagination.reset;
  useEffect(() => {
    resetCursor();
  }, [search, resetCursor]);
  const [draft, setDraft] = useState(() => filterDraft(filters));
  const [prevFilters, setPrevFilters] = useState(filters);
  if (prevFilters !== filters) {
    setPrevFilters(filters);
    setDraft(filterDraft(filters));
  }
  const query = useLogs({ ...filters, cursor: cursorPagination.cursor });
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
  useEffect(() => { if (inspectorOpen && !isMobile) inspectorClose.current?.focus(); }, [inspectorOpen, isMobile]);
  const prefix = idSearch.trim().toLowerCase();
  const rows = query.data?.items ?? [];
  const matchingRows = rows.filter(row => row.id.toLowerCase().startsWith(prefix));
  const index = matchingRows.findIndex(row => row.id === selectedId);
  const detail = useRequestLog(basePath, selectedId);

  const appliedUser = useUser(isSystemView && filters.user_id ? filters.user_id : "").data?.data;
  const appliedAdminKey = useAdminApiKey(isSystemView && filters.api_key_id ? filters.api_key_id : "").data?.data;
  const appliedOwnKey = useOwnApiKey(!isSystemView && filters.api_key_id ? filters.api_key_id : "").data?.data;
  const draftAdminKey = useAdminApiKey(isSystemView && draft.api_key_id ? draft.api_key_id : "").data?.data;
  const appliedApiKey = isSystemView ? appliedAdminKey : appliedOwnKey;
  const appliedApiKeyLabel = appliedApiKey
    ? (isSystemView && appliedUser ? `${appliedApiKey.name} · ${appliedUser.display_name}` : appliedApiKey.name)
    : undefined;
  const filterLabels: [string, string | undefined][] = [
    ["User", isSystemView && filters.user_id ? (appliedUser?.display_name ?? t("Unavailable")) : undefined],
    ["API key", appliedApiKeyLabel ?? (filters.api_key_id ? t("Unavailable") : undefined)],
    ["Requested model", filters.model],
    ["API format", filters.api_format && apiFormatLabel(filters.api_format)],
    ["Operation", filters.api_operation && apiOperationLabel(filters.api_operation)],
    ["Outcome", filters.outcome && outcomeLabel(filters.outcome)],
    ["Billing", filters.billed === undefined ? undefined : t(filters.billed ? "Billed" : "Unbilled")],
    ["From", filters.started_after && formatDateTime(filters.started_after)],
    ["To", filters.started_before && formatDateTime(filters.started_before)],
    ["Rows per page", filters.limit !== undefined && filters.limit !== emptyFilters.limit
      ? t("{count} per page", { count: filters.limit })
      : undefined],
  ];
  const appliedConditions = filterLabels.flatMap(([label, value]) => value ? [{ label, value }] : []);

  const updateDraft = (partial: Partial<RequestLogFilterDraft>) => {
    setDraft((previous) => ({ ...previous, ...partial }));
  };
  const runQuery = (next: AdminRequestLogQuery) => {
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
      basis: 7,
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
      basis: 9,
      render: (log) => (
        <div aria-current={selectedId === log.id || undefined}>
          <RequestModel log={log} compact />
        </div>
      ),
    },
    {
      key: "operation-protocol",
      header: t("Operation / protocol"),
      basis: 6.5,
      render: (log) => (
        <span className="flex flex-col items-start gap-1">
          <span className="font-medium">
            {apiOperationLabel(log.api_operation)}
          </span>
          <span className="text-xs text-muted-foreground">
            {requestProtocolLabel(log.request_protocol, t)}
          </span>
        </span>
      ),
    },
    {
      key: "channel-group",
      header: t("Channel group"),
      basis: 6,
      render: (log) => <TruncatedName value={log.channel_group_name} />,
    },
    ...(isSystemView
      ? [
          {
            key: "channel",
            header: t("Channel"),
            basis: 5,
            render: (log: RequestLogView) => <TruncatedName value={log.channel_name} />,
          },
          {
            key: "user",
            header: t("User"),
            basis: 5,
            render: (log: RequestLogView) => <TruncatedName value={log.user_name} />,
          },
        ]
      : []),
    {
      key: "outcome",
      header: t("Outcome"),
      basis: 6,
      render: (log) => (
        <Badge variant={outcomeVariant(log.outcome)}>{outcomeLabel(log.outcome)}</Badge>
      ),
    },
    {
      key: "tokens",
      header: t("Tokens"),
      basis: 6.5,
      render: (log) => <TokenUsage log={log} />,
    },
    {
      key: "cost",
      header: t("Cost"),
      basis: 5.25,
      render: (log) => <RequestCost log={log} />,
    },
    {
      key: "duration",
      header: t("Duration"),
      basis: 5.25,
      className: "text-right",
      render: (log) => <RequestDuration log={log} />,
    },
  ];
  const compactColumns: Column<RequestLogView>[] = [
    {
      key: "request",
      header: t("Request log"),
      basis: 8,
      render: (log) => (
        <div className="flex flex-col items-start gap-1">
          <time dateTime={log.started_at} className="text-xs text-muted-foreground">
            {formatDateTime(log.started_at)}
          </time>
          <RequestModel log={log} compact />
          <Badge variant={outcomeVariant(log.outcome)}>{outcomeLabel(log.outcome)}</Badge>
        </div>
      ),
    },
  ];

  return (
    <Collapsible
      className="@container flex min-w-0 flex-col gap-3"
      open={filtersOpen}
      onOpenChange={setFiltersOpen}
    >
      <PageHeader title={title} actions={
        <ToggleGroup
          size="sm" variant="outline" spacing={0}
          className="max-w-full flex-wrap"
          aria-label={t("Outcome")} value={[filters.outcome ?? "all"]}
          onValueChange={([value]) => {
            if (value) runQuery({ ...filters, outcome: OUTCOMES.find(outcome => outcome === value) });
          }}
        >
          <ToggleGroupItem value="all">{t("All")}</ToggleGroupItem>
          {OUTCOMES.map(outcome => (
            <ToggleGroupItem key={outcome} value={outcome}>{outcomeLabel(outcome)}</ToggleGroupItem>
          ))}
        </ToggleGroup>
      } />
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex items-center gap-2 border-b pb-2">
          <form className="mr-auto min-w-0 max-w-72 flex-1" onSubmit={e => {
            e.preventDefault(); const id = idSearch.trim(); if (uuid.test(id)) setSelectedId(id);
          }}>
            <InputGroup>
              <InputGroupInput aria-label={t("Find request ID")} placeholder={t("Find request ID")} value={idSearch} type="search" autoComplete="off" spellCheck={false} onChange={e => setIdSearch(e.target.value)} />
              <InputGroupAddon><Search aria-hidden /></InputGroupAddon>
              {uuid.test(idSearch.trim()) && <InputGroupAddon align="inline-end"><InputGroupButton type="submit">{t("View details")}</InputGroupButton></InputGroupAddon>}
            </InputGroup>
          </form>
          <CollapsibleTrigger
            ref={filterToggle}
            render={<Button variant="outline" size="sm" className="self-start scroll-mt-20" />}
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
                  <UserFilterCombobox
                    id="request-log-user"
                    ariaLabel={t("User")}
                    value={draft.user_id}
                    onValueChange={(userId) => {
                      updateDraft({
                        user_id: userId,
                        api_key_id:
                          draft.api_key_id &&
                          (!userId || draftAdminKey?.user_id === userId)
                            ? draft.api_key_id
                            : "",
                      });
                    }}
                  />
                </Field>
              ) : null}
              <Field>
                <FieldLabel htmlFor="request-log-api-key">{t("API key")}</FieldLabel>
                <ApiKeyFilterCombobox
                  id="request-log-api-key"
                  ariaLabel={t("API key")}
                  scope={scope}
                  userId={isSystemView ? draft.user_id : undefined}
                  value={draft.api_key_id}
                  onValueChange={(apiKeyId) => updateDraft({ api_key_id: apiKeyId })}
                />
              </Field>
              <Field className={cn(!isSystemView && "xl:col-span-2")}>
                <FieldLabel htmlFor="request-log-model">{t("Requested model")}</FieldLabel>
                <Combobox
                  items={modelOptions.data ?? []}
                  inputValue={draft.model}
                  value={draft.model || null}
                  onInputValueChange={(model) => updateDraft({ model })}
                >
                  <ComboboxInput
                    id="request-log-model"
                    aria-label={t("Requested model")}
                    aria-describedby="request-log-model-help"
                    placeholder={t("All models")}
                    spellCheck={false}
                  />
                  <ComboboxContent>
                    <ComboboxEmpty>
                      {modelOptions.isLoading ? t("Loading…") : modelOptions.isError ? t("Unavailable") : t("No matching suggestions")}
                    </ComboboxEmpty>
                    <ComboboxList>
                      {(model: string) => <ComboboxItem key={model} value={model}>{model}</ComboboxItem>}
                    </ComboboxList>
                  </ComboboxContent>
                </Combobox>
                <FieldDescription id="request-log-model-help">
                  {t("Choose a priced model or enter the exact model ID from the request.")}
                </FieldDescription>
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
                          {apiOperationLabel(operation)}
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
                <FieldLabel htmlFor="request-log-limit">{t("Rows per page")}</FieldLabel>
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
                          {t("{count} per page", { count: value })}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field className="justify-end">
                <FieldLabel className="sr-only">{t("Filter actions")}</FieldLabel>
                <div className="flex min-w-0 flex-wrap gap-2">
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
          </form>
      </CollapsibleContent>
      <div className={cn("grid min-w-0 items-start", selectedId && !isMobile && "@min-[44rem]:grid-cols-[minmax(0,1fr)_minmax(16rem,31%)]")}>
        <section aria-label={t("Requests")} className="min-w-0 [&_tr:has([aria-current=true])]:bg-accent/60">
          <AsyncResource
            isLoading={query.isLoading}
            error={query.error}
            isEmpty={matchingRows.length === 0}
            emptyTitle={t("No requests match these filters")}
          >
            <ResourceTable
              columns={isMobile ? compactColumns : columns.filter(col => !hiddenColumns.has(col.key))}
              rows={matchingRows}
              rowKey={(log) => log.id}
              onRowClick={(log) => setSelectedId(log.id)}
              rowActionLabel={t("View details")}
              pagination={false}
            />
            <CursorPagination
              page={cursorPagination.page}
              pageSize={filters.limit ?? 50}
              pageSizeOptions={LIMITS}
              onPageSizeChange={(limit) => runQuery({ ...filters, limit })}
              hasPrevious={cursorPagination.hasPrevious}
              hasNext={Boolean(query.data?.next_cursor)}
              onPrevious={cursorPagination.goPrevious}
              onNext={() => cursorPagination.goNext(query.data?.next_cursor)}
              itemCount={rows.length}
              isFetching={query.isFetching}
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
                value={apiOperationLabel(detail.data.api_operation)}
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
