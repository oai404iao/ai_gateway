import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import {
  ApiKeyTargetFields,
  type ApiKeyTargetChannel,
  type ApiKeyTargetGroup,
} from "@/components/shared/api-key-target-fields";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ApiKeyValue } from "@/components/shared/api-key-value";
import { DecimalField, NullableNumberField } from "@/components/shared/decimal-field";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useCreateOwnApiKey,
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
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

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
  const { data: keys, isLoading, error } = useOwnApiKeys();
  const create = useCreateOwnApiKey();
  const { t } = useI18n();
  const [createOpen, setCreateOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const options = useOwnApiKeyOptions(createOpen);

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

  const targetGroups = useMemo<ApiKeyTargetGroup[]>(
    () => options.data?.groups ?? [],
    [options.data?.groups],
  );
  const targetChannels = useMemo<ApiKeyTargetChannel[]>(
    () => options.data?.channels ?? [],
    [options.data?.channels],
  );
  const selectedGroupIds = form.watch("allowed_group_ids");
  const selectedChannelIds = form.watch("allowed_channel_ids");
  const targetError =
    form.formState.errors.allowed_group_ids?.message ??
    form.formState.errors.allowed_channel_ids?.message;

  const columns: Column<ApiKeyView>[] = [
    {
      key: "name",
      header: t("Name"),
      render: (key) => <span className="font-medium">{key.name}</span>,
    },
    {
      key: "status",
      header: t("Status"),
      render: (key) => <StatusBadge value={key.status} />,
    },
    {
      key: "secret",
      header: t("API key"),
      render: (key) => <ApiKeyValue value={key.secret} />,
      className: "min-w-80",
    },
    {
      key: "formats",
      header: t("Formats"),
      render: (key) => (
        <span className="flex flex-wrap gap-1">
          {key.allowed_api_formats.map((format) => (
            <StatusBadge
              key={format}
              value={format}
              label={apiFormatLabel(format)}
              variant="info"
            />
          ))}
        </span>
      ),
    },
    {
      key: "permissions",
      header: t("Permissions"),
      render: (key) => formatList(key.permissions),
    },
    {
      key: "expiry",
      header: t("Expires"),
      render: (key) => formatExpiry(key.expires_at),
    },
    {
      key: "updated",
      header: t("Updated"),
      render: (key) => formatRelative(key.updated_at),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("API Keys")}
        description={t("Your personal client keys for the OpenAI-compatible data plane.")}
        actions={
          <Button onClick={() => setCreateOpen(true)}>
            <Plus data-icon="inline-start" /> {t("New API key")}
          </Button>
        }
      />
      <Card>
        <CardHeader>
          <CardTitle>{t("Keys")}</CardTitle>
          <CardDescription>
            {t(
              "Each key has its own routing targets, rate limits, concurrency limit, and quota.",
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={keys?.length === 0}
            emptyTitle={t("No API keys")}
            emptyDescription={t("Create your first API key to start using the data plane.")}
          >
            <ResourceTable
              columns={columns}
              rows={keys ?? []}
              rowKey={(key) => key.id}
              onRowClick={(key) => navigate(`/api-keys/${key.id}`)}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      <Dialog
        open={createOpen}
        onOpenChange={(open) => {
          setCreateOpen(open);
          if (!open) form.reset();
        }}
      >
        <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{t("New API key")}</DialogTitle>
            <DialogDescription>
              {t(
                "Choose targets from your policy and configure this key's own limits.",
              )}
            </DialogDescription>
          </DialogHeader>
          <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
            <FieldGroup>
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
                <FieldError>{createErrorMessage(options.error, t)}</FieldError>
              ) : null}
              <ApiKeyTargetFields
                groups={targetGroups}
                channels={targetChannels}
                selectedGroupIds={selectedGroupIds}
                selectedChannelIds={selectedChannelIds}
                onChange={(allowedGroupIds, allowedChannelIds) => {
                  form.setValue("allowed_group_ids", allowedGroupIds, {
                    shouldDirty: true,
                    shouldValidate: true,
                  });
                  form.setValue("allowed_channel_ids", allowedChannelIds, {
                    shouldDirty: true,
                    shouldValidate: true,
                  });
                }}
                error={targetError ? t(targetError) : undefined}
              />
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
            <DialogFooter>
              <Button type="submit" disabled={submitting || options.isLoading || Boolean(options.error)}>
                {submitting || options.isLoading ? <Spinner data-icon="inline-start" /> : null}
                {t("Create key")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}
