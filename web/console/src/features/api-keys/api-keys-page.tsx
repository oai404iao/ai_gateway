import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";
import { useListPagination } from "@/lib/use-list-pagination";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Check, Copy, Ellipsis, Plus, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetFooter, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import {
  RoutingTargetFields,
  type RoutingTargetChannel,
  type RoutingTargetGroup,
} from "@/components/shared/routing-target-fields";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ApiKeyValue } from "@/components/shared/api-key-value";
import { DecimalField, NullableNumberField } from "@/components/shared/decimal-field";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useCreateOwnApiKey,
  useApiHosts,
  useOwnApiKeyOptions,
  useOwnApiKeys,
} from "@/features/api-keys/api";
import type { ApiKeyView } from "@/api/types";
import { ApiError } from "@/api/errors";
import {
  dateTimeLocalToIso,
  formatExpiry,
  formatRelative,
  isFutureDateTimeLocal,
} from "@/lib/dates";
import { formatList } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";
import { API_KEY_STATUSES } from "@/lib/permissions";
import { useIsMobile } from "@/hooks/use-mobile";

const createSchema = z
  .object({
    name: z.string().min(1, "Name is required.").max(100),
    expires_at: z
      .string()
      .optional()
      .refine(isFutureDateTimeLocal, "Expiry must be a valid future date and time."),
    allowed_group_ids: z.array(z.string()),
    allowed_channel_ids: z.array(z.string()),
    requests_per_minute: z.number().int().positive().nullable(),
    max_concurrent_requests: z.number().int().positive().nullable(),
    quota_limit_amount: z
      .string()
      .regex(/^\d+(?:\.\d+)?$/, "Enter a non-negative amount.")
      .nullable(),
  })
  .superRefine((value, context) => {
    if (value.allowed_group_ids.length === 0 && value.allowed_channel_ids.length === 0) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["allowed_group_ids"],
        message: "Pick at least one channel group or channel.",
      });
    }
  });

type CreateValues = z.infer<typeof createSchema>;

function createErrorMessage(error: unknown, t: (key: string) => string): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "default_api_key_policy_required":
        return t("Create an API key policy, then assign it to this user under Administration → Users.");
      case "default_api_key_policy_disabled":
        return t("Your default API key policy is disabled. Ask an administrator to enable or replace it.");
      case "api_key_target_not_allowed":
        return t("One or more selected targets are no longer allowed by your API key policy.");
      default:
        return error.message;
    }
  }
  return error instanceof Error ? error.message : t("Create failed");
}

export function ApiKeysPage() {
  const navigate = useNavigate();
  const origin = usePageOrigin();
  const pagination = useListPagination();
  const isMobile = useIsMobile();
  const { data: keys, isLoading, error } = useOwnApiKeys();
  const create = useCreateOwnApiKey();
  const apiHosts = useApiHosts();
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  const createOpen = params.get("create") === "1";
  const setCreateOpen = (open: boolean) => {
    const next = new URLSearchParams(params);
    if (open) next.set("create", "1"); else next.delete("create");
    setParams(next, { replace: true, preventScrollReset: true });
  };
  const [search, setSearch] = useState("");
  const activeStatus = API_KEY_STATUSES.find(value => value === params.get("status")) ?? "";
  const needle = search.trim().toLocaleLowerCase();
  const matched = (keys ?? []).filter(key =>
    (!activeStatus || key.status === activeStatus) && key.name.toLocaleLowerCase().includes(needle),
  );
  const selectStatus = (status: string) => {
    const next = new URLSearchParams(params);
    next.delete("page");
    if (status) next.set("status", status); else next.delete("status");
    setParams(next, { replace: true, preventScrollReset: true });
  };
  const [submitting, setSubmitting] = useState(false);
  const [copiedHost, setCopiedHost] = useState<string | null>(null);
  const copiedHostTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (copiedHostTimeout.current !== null) {
        clearTimeout(copiedHostTimeout.current);
      }
    },
    [],
  );
  const options = useOwnApiKeyOptions();

  const form = useForm<CreateValues>({
    resolver: zodResolver(createSchema),
    defaultValues: {
      name: "",
      expires_at: "",
      allowed_group_ids: [],
      allowed_channel_ids: [],
      requests_per_minute: null,
      max_concurrent_requests: null,
      quota_limit_amount: null,
    },
  });

  const onSubmit = async (values: CreateValues) => {
    setSubmitting(true);
    try {
      await create.mutateAsync({
        name: values.name,
        expires_at: dateTimeLocalToIso(values.expires_at),
        allowed_group_ids: values.allowed_group_ids,
        allowed_channel_ids: values.allowed_channel_ids,
        requests_per_minute: values.requests_per_minute,
        max_concurrent_requests: values.max_concurrent_requests,
        quota_limit_amount: values.quota_limit_amount,
      });
      setCreateOpen(false);
      form.reset();
      toast.success(t("API key created"));
    } catch (error) {
      toast.error(createErrorMessage(error, t));
    } finally {
      setSubmitting(false);
    }
  };

  const targetGroups = useMemo<RoutingTargetGroup[]>(
    () => options.data?.groups ?? [],
    [options.data?.groups],
  );
  const targetChannels = useMemo<RoutingTargetChannel[]>(
    () => options.data?.channels ?? [],
    [options.data?.channels],
  );
  const selectedGroupIds = form.watch("allowed_group_ids");
  const selectedChannelIds = form.watch("allowed_channel_ids");
  const targetError =
    form.formState.errors.allowed_group_ids?.message ??
    form.formState.errors.allowed_channel_ids?.message;

  const copyApiHost = async (apiHost: string) => {
    try {
      await navigator.clipboard.writeText(apiHost);
      setCopiedHost(apiHost);
      toast.success(t("API host copied"));
      if (copiedHostTimeout.current !== null) {
        clearTimeout(copiedHostTimeout.current);
      }
      copiedHostTimeout.current = setTimeout(() => {
        copiedHostTimeout.current = null;
        setCopiedHost(null);
      }, 2000);
    } catch {
      toast.error(t("Copy failed"));
    }
  };

  const columns: Column<ApiKeyView>[] = [
    {
      key: "name",
      basis: 8,
      header: t("Name"),
      render: (key) => <span className="font-medium">{key.name}</span>,
    },
    {
      key: "status",
      basis: 4.5,
      header: t("Status"),
      render: (key) => <StatusBadge value={key.status} />,
    },
    {
      key: "secret",
      basis: 11.5,
      header: t("API key"),
      render: (key) => <ApiKeyValue value={key.secret} className="min-w-0" />,
    },
    {
      key: "groups",
      basis: 7,
      header: t("Channel groups"),
      render: (key) => formatList(key.allowed_group_ids.map((id) =>
        targetGroups.find((group) => group.id === id)?.name ?? t("Unavailable channel group"))),
    },
    {
      key: "channels",
      basis: 8,
      header: t("Logical channels"),
      render: (key) => formatList(key.allowed_channel_ids.map((id) => {
        const channel = targetChannels.find((channel) => channel.id === id);
        if (!channel) return t("Unknown channel");
        const groupName = channel.channel_group_name ?? targetGroups.find(group => group.id === channel.channel_group_id)?.name;
        return groupName ? `${channel.name} (${groupName})` : channel.name;
      })),
    },
    {
      key: "permissions",
      basis: 7,
      header: t("Permissions"),
      render: (key) => formatList(key.permissions),
    },
    {
      key: "expiry",
      basis: 6.5,
      header: t("Expires"),
      render: (key) => formatExpiry(key.expires_at),
    },
    {
      key: "updated",
      basis: 5.5,
      header: t("Updated"),
      render: (key) => formatRelative(key.updated_at),
    },
    {
      key: "actions", header: t("Actions"), pinned: true, basis: 3.75,
      render: key => <DropdownMenu><DropdownMenuTrigger aria-label={t("Actions for {name}", { name: key.name })} render={<Button variant="ghost" size="icon-sm" />}><Ellipsis /></DropdownMenuTrigger>
        <DropdownMenuContent align="end" onClick={event => event.stopPropagation()}><DropdownMenuGroup>
          <DropdownMenuItem onClick={() => navigate(withReturnTo(`/api-keys/${key.id}`, origin))}>{t("View details")}</DropdownMenuItem>
          {key.status !== "revoked" && <DropdownMenuItem onClick={() => navigate(withReturnTo(`/api-keys/${key.id}?edit=1`, origin))}>{t("Edit")}</DropdownMenuItem>}
        </DropdownMenuGroup></DropdownMenuContent></DropdownMenu>,
    },
  ];

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <PageHeader
        title={t("API Keys")}
        actions={
          <Button onClick={() => setCreateOpen(true)}>
            <Plus data-icon="inline-start" /> {t("New API key")}
          </Button>
        }
      />
      <div role="group" aria-label={t("Status")} className="flex min-w-0 gap-2 overflow-x-auto border-b">
        {(["", ...API_KEY_STATUSES] as const).map(value => <button key={value} type="button" aria-pressed={value === activeStatus}
          className="inline-flex shrink-0 items-center gap-1.5 border-b-2 border-transparent px-2 py-2 text-xs font-medium text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring aria-pressed:border-primary aria-pressed:text-foreground"
          onClick={() => selectStatus(value)}>
          {value ? t(value === "active" ? "Active" : value === "disabled" ? "Disabled" : "Revoked") : t("All")}
          {keys && <> <span className="font-mono text-[10px] text-muted-foreground">{keys.filter(key => !value || key.status === value).length}</span></>}
        </button>)}
      </div>
      <label className="flex max-w-72 items-center gap-1 text-muted-foreground"><Search className="size-4 shrink-0" /><Input type="search" value={search} aria-label={t("Filter key names")} placeholder={t("Filter key names")} className="h-8 border-0 px-1 shadow-none" onChange={e => { setSearch(e.target.value); if (pagination.page !== 1) pagination.onPageChange(1); }} /></label>
      <section aria-label={t("Keys")} className="min-w-0">
        <AsyncResource isLoading={isLoading} error={error} isEmpty={!matched.length}
          emptyTitle={t(keys?.length ? "No matching API keys" : "No API keys")}
          emptyDescription={t(keys?.length ? "Try another key name or status." : "Create an API key to send requests.")}>
          <ResourceTable columns={isMobile ? columns.filter(col => ["name", "status", "actions"].includes(col.key)) : columns} rows={matched} rowKey={key => key.id}
            rowHref={key => withReturnTo(`/api-keys/${key.id}`, origin)} onRowClick={key => navigate(withReturnTo(`/api-keys/${key.id}`, origin))} pagination={pagination} />
        </AsyncResource>
      </section>
      <section className="mt-3 grid min-w-0 gap-3 border-t py-4 md:grid-cols-[minmax(12rem,1fr)_minmax(0,2fr)]">
        <h2 className="text-sm font-semibold">{t("API hosts")}</h2>
        <div>
          <AsyncResource
            isLoading={apiHosts.isLoading}
            error={apiHosts.error}
            isEmpty={apiHosts.data?.api_hosts.length === 0}
            emptyTitle={t("No API hosts configured")}
            emptyDescription={t("Ask an administrator to configure an API host in System settings.")}
          >
            <div className="flex flex-col gap-2">
              {(apiHosts.data?.api_hosts ?? []).map((apiHost) => (
                <InputGroup key={apiHost}>
                  <InputGroupInput
                    readOnly
                    value={apiHost}
                    aria-label={t("API host")}
                    className="font-mono text-xs"
                  />
                  <InputGroupAddon align="inline-end">
                    <InputGroupButton
                      aria-label={t("Copy API host")}
                      onClick={() => void copyApiHost(apiHost)}
                    >
                      {copiedHost === apiHost ? (
                        <Check data-icon="inline-start" />
                      ) : (
                        <Copy data-icon="inline-start" />
                      )}
                      {t(copiedHost === apiHost ? "Copied" : "Copy")}
                    </InputGroupButton>
                  </InputGroupAddon>
                </InputGroup>
              ))}
            </div>
          </AsyncResource>
        </div>
      </section>

      <Sheet
        open={createOpen}
        onOpenChange={(open) => {
          if (submitting) return;
          setCreateOpen(open);
          if (!open) form.reset();
        }}
      >
        <SheetContent className="overflow-y-auto data-[side=right]:w-full data-[side=right]:sm:max-w-2xl">
          <SheetHeader className="border-b pr-12">
            <SheetTitle>{t("New API key")}</SheetTitle>
          </SheetHeader>
          <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4 px-4">
            <FieldGroup className="grid gap-5 md:grid-cols-2">
              <Field data-invalid={Boolean(form.formState.errors.name)}>
                <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                <Input
                  id="name"
                  aria-invalid={Boolean(form.formState.errors.name)}
                  {...form.register("name")}
                />
                {form.formState.errors.name ? (
                  <FieldError>{t(form.formState.errors.name.message ?? "")}</FieldError>
                ) : null}
              </Field>
              <Field data-invalid={Boolean(form.formState.errors.expires_at)}>
                <FieldLabel htmlFor="expires_at">{t("Expires at (optional)")}</FieldLabel>
                <Input
                  id="expires_at"
                  type="datetime-local"
                  aria-invalid={Boolean(form.formState.errors.expires_at)}
                  {...form.register("expires_at")}
                />
                {form.formState.errors.expires_at ? (
                  <FieldError>{t(form.formState.errors.expires_at.message ?? "")}</FieldError>
                ) : null}
              </Field>
              {options.error ? (
                <FieldError className="md:col-span-2">
                  {createErrorMessage(options.error, t)}
                </FieldError>
              ) : null}
              <div className="md:col-span-2">
                <RoutingTargetFields
                  groups={targetGroups}
                  channels={targetChannels}
                  selectedGroupIds={selectedGroupIds}
                  selectedChannelIds={selectedChannelIds}
                  onChange={(allowedGroupIds, allowedChannelIds) => {
                    form.setValue("allowed_group_ids", allowedGroupIds, {
                      shouldDirty: true,
                      shouldValidate: true,
                    });
                    form.setValue(
                      "allowed_channel_ids",
                      allowedChannelIds,
                      { shouldDirty: true, shouldValidate: true },
                    );
                  }}
                  legend={t("API Key Policy targets")}
                  description={options.data?.policy_enabled === false
                    ? t("Ask an administrator to assign an active API key policy.") : undefined}
                  error={targetError ? t(targetError) : undefined}
                />
              </div>
              <NullableNumberField
                id="requests_per_minute"
                label={t("Requests / minute")}
                value={form.watch("requests_per_minute")}
                onChange={(value) =>
                  form.setValue("requests_per_minute", value, {
                    shouldDirty: true,
                    shouldValidate: true,
                  })
                }
                error={
                  form.formState.errors.requests_per_minute?.message
                    ? t(form.formState.errors.requests_per_minute.message)
                    : undefined
                }
              />
              <NullableNumberField
                id="max_concurrent_requests"
                label={t("Max concurrent requests")}
                value={form.watch("max_concurrent_requests")}
                onChange={(value) =>
                  form.setValue("max_concurrent_requests", value, {
                    shouldDirty: true,
                    shouldValidate: true,
                  })
                }
                error={
                  form.formState.errors.max_concurrent_requests?.message
                    ? t(form.formState.errors.max_concurrent_requests.message)
                    : undefined
                }
              />
              <DecimalField
                id="quota_limit_amount"
                label={t("Quota limit amount")}
                value={form.watch("quota_limit_amount")}
                onChange={(value) =>
                  form.setValue("quota_limit_amount", value || null, {
                    shouldDirty: true,
                    shouldValidate: true,
                  })
                }
                error={
                  form.formState.errors.quota_limit_amount?.message
                    ? t(form.formState.errors.quota_limit_amount.message)
                    : undefined
                }
                description={t("Leave blank for no per-key quota limit.")}
              />
            </FieldGroup>
            <SheetFooter className="sticky bottom-0 flex-row justify-end border-t bg-background px-0">
              <Button type="submit" disabled={submitting || options.isLoading || Boolean(options.error)}>
                {submitting || options.isLoading ? <Spinner data-icon="inline-start" /> : null}
                {t("Create key")}
              </Button>
            </SheetFooter>
          </form>
        </SheetContent>
      </Sheet>
    </div>
  );
}
