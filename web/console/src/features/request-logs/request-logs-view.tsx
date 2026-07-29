import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, Search, X } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
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
import { dateTimeLocalToIso, formatDateTime, formatRelative } from "@/lib/dates";
import { formatDurationMs, formatTokens, formatUsd } from "@/lib/formatters";
import { API_FORMATS, apiFormatLabel, outcomeLabel, outcomeVariant } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

const LIMITS = [25, 50, 100] as const;
const OUTCOMES = ["succeeded", "failed", "rejected", "cancelled"] as const;

type ApiFormatFilter = NonNullable<ListQuery["api_format"]>;
type OutcomeFilter = NonNullable<ListQuery["outcome"]>;

interface RequestLogListResult {
  data: RequestLogView[] | undefined;
  isLoading: boolean;
  error: unknown;
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
  outcome: "",
  started_after: "",
  started_before: "",
  billed: "",
};

interface RequestLogsViewProps {
  title: string;
  description: string;
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

function TokenUsage({ log }: { log: RequestLogView }) {
  const { t } = useI18n();
  const uncachedInput = tokenRemainder(log.input_tokens, log.cached_input_tokens);
  const nonReasoningOutput = tokenRemainder(log.output_tokens, log.reasoning_tokens);
  const uncachedInputLabel = `${t("Uncached input")}: ${formatTokens(uncachedInput)}`;
  const cachedInputLabel = `${t("Cached input")}: ${formatTokens(log.cached_input_tokens)}`;
  const nonReasoningOutputLabel = `${t("Non-reasoning output")}: ${formatTokens(nonReasoningOutput)}`;
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
          aria-label={nonReasoningOutputLabel}
          title={nonReasoningOutputLabel}
        >
          <ArrowDown className="size-4 shrink-0 text-info" aria-hidden />
          <span>{formatTokens(nonReasoningOutput)}</span>
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

function RequestDuration({ log }: { log: RequestLogView }) {
  const { t } = useI18n();
  const ttft = formatDurationMs(log.ttft_ms);
  const total = formatDurationMs(log.total_duration_ms);
  const ttftLabel = `${t("TTFT")}: ${ttft}`;
  const totalLabel = `${t("Total duration")}: ${total}`;

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
    outcome: draft.outcome || undefined,
    started_after: dateTimeLocalToIso(draft.started_after) ?? undefined,
    started_before: dateTimeLocalToIso(draft.started_before) ?? undefined,
    billed: draft.billed === "" ? undefined : draft.billed === "true",
  };
}

export function RequestLogsView({
  title,
  description,
  basePath,
  useLogs,
  scope,
  users = [],
  apiKeys = [],
  modelOptions = [],
}: RequestLogsViewProps) {
  const { t } = useI18n();
  const isSystemView = scope === "system";
  const [draft, setDraft] = useState<RequestLogFilterDraft>(emptyFilters);
  const [filters, setFilters] = useState<ListQuery>(() => toQuery(emptyFilters));
  const query = useLogs(filters);
  const [selectedId, setSelectedId] = useState<string | null>(null);
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

  const updateDraft = (partial: Partial<RequestLogFilterDraft>) => {
    setDraft((previous) => ({ ...previous, ...partial }));
  };
  const applyFilters = () => setFilters(toQuery(draft));
  const clearFilters = () => {
    setDraft(emptyFilters);
    setFilters(toQuery(emptyFilters));
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
      render: (log) => <span className="font-medium">{log.client_model}</span>,
    },
    {
      key: "protocol",
      header: t("Protocol"),
      render: (log) => (
        <Badge variant="outline">{requestProtocolLabel(log.request_protocol, t)}</Badge>
      ),
    },
    {
      key: "channel-group",
      header: t("Channel group"),
      render: (log) => log.channel_group_name ?? "—",
    },
    ...(isSystemView
      ? [
          {
            key: "channel",
            header: t("Channel"),
            render: (log: RequestLogView) => log.channel_name ?? "—",
          },
          {
            key: "user",
            header: t("User"),
            render: (log: RequestLogView) => log.user_name ?? "—",
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
      render: (log) => formatUsd(log.cost_amount),
    },
    {
      key: "duration",
      header: t("Duration"),
      render: (log) => <RequestDuration log={log} />,
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={title} description={description} />
      <Card size="sm">
        <CardHeader className="border-b">
          <CardTitle>{t("Filters")}</CardTitle>
          <CardDescription>
            {t("Filter by exact model, request outcome, format, time range, and settlement state.")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              applyFilters();
            }}
          >
            <FieldGroup className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-6">
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
                  <SelectTrigger id="request-log-model" className="w-full">
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
                  value={draft.started_after}
                  onChange={(event) => updateDraft({ started_after: event.target.value })}
                />
              </Field>
              <Field className="xl:col-span-2">
                <FieldLabel htmlFor="request-log-started-before">{t("To")}</FieldLabel>
                <Input
                  id="request-log-started-before"
                  type="datetime-local"
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
                  <Button type="submit" className="flex-1">
                    <Search data-icon="inline-start" />
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
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>{t("Requests")}</CardTitle>
          <CardDescription>
            {t(
              "The gateway does not store request or response bodies. Sanitized upstream error messages may be retained.",
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={query.isLoading}
            error={query.error}
            isEmpty={query.data?.length === 0}
            emptyTitle={t("No request logs")}
            emptyDescription={t("There are no logged requests matching these filters.")}
          >
            <ResourceTable
              columns={columns}
              rows={query.data ?? []}
              rowKey={(log) => log.id}
              onRowClick={(log) => setSelectedId(log.id)}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      <Sheet open={Boolean(selectedId)} onOpenChange={(open) => !open && setSelectedId(null)}>
        <SheetContent className="overflow-y-auto sm:max-w-md">
          <SheetHeader>
            <SheetTitle>{t("Request log")}</SheetTitle>
            <SheetDescription>
              {detail.data ? formatDateTime(detail.data.started_at) : t("Loading…")}
            </SheetDescription>
          </SheetHeader>
          {detail.data ? (
            <dl className="grid grid-cols-1 gap-3 p-4">
              <DetailField label={t("Started")} value={formatDateTime(detail.data.started_at)} />
              <DetailField label={t("Model")} value={detail.data.client_model} mono />
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
              <DetailField label={t("Cost")} value={formatUsd(detail.data.cost_amount)} />
              <DetailField
                label={t("Duration")}
                value={formatDurationMs(detail.data.total_duration_ms)}
              />
              <DetailField label={t("HTTP")} value={detail.data.response_status_code ?? "—"} />
              <DetailField label={t("Error code")} value={detail.data.error_code ?? "—"} mono />
              <DetailField
                label={t("Error message")}
                value={
                  <span className="whitespace-pre-wrap">
                    {detail.data.error_summary ?? "—"}
                  </span>
                }
              />
              <DetailField label={t("Completed")} value={formatDateTime(detail.data.completed_at)} />
            </dl>
          ) : null}
        </SheetContent>
      </Sheet>
    </div>
  );
}
