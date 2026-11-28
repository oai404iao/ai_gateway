import { useCallback, useEffect, useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Field, FieldError, FieldSet, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { useReturnPath } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/hooks/use-configuration-draft";
import { ApiKeyValue } from "@/components/shared/api-key-value";
import {
  RoutingTargetFields,
  type RoutingTargetChannel,
  type RoutingTargetGroup,
} from "@/components/shared/routing-target-fields";
import { DecimalField, NullableNumberField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import {
  useDeleteOwnApiKey,
  useOwnApiKey,
  useOwnApiKeyOptions,
  useRevokeOwnApiKey,
  useUpdateOwnApiKey,
} from "@/features/api-keys/api";
import { ApiError } from "@/api/errors";
import type { ApiKeyView } from "@/api/types";
import { formatList, formatUsd } from "@/lib/formatters";
import { resourceLabels } from "@/lib/resource-labels";
import {
  dateTimeLocalToIso,
  formatDateTime,
  formatDateTimeLocalInput,
  formatExpiry,
} from "@/lib/dates";
import { API_KEY_STATUSES } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const editSchema = z
  .object({
    name: z.string().min(1, "Name is required.").max(100),
    status: z.enum(["active", "disabled"]),
    expires_at: z.string().optional(),
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

type EditValues = z.infer<typeof editSchema>;
type KeyVersion = { data: ApiKeyView; etag: string };

const emptyEditValues: EditValues = {
  name: "",
  status: "active",
  expires_at: "",
  allowed_group_ids: [],
  allowed_channel_ids: [],
  requests_per_minute: null,
  max_concurrent_requests: null,
  quota_limit_amount: null,
};

export function ApiKeyDetailPage() {
  const { id = "" } = useParams();
  return <ApiKeyDetail key={id} id={id} />;
}

function ApiKeyDetail({ id }: { id: string }) {
  const returnTo = useReturnPath("/api-keys");
  const { data, isLoading, isFetching, error, refetch } = useOwnApiKey(id);
  const [baseline, setBaseline] = useState<KeyVersion>();
  const [conflict, setConflict] = useState(false);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [params] = useSearchParams();
  const [editing, setEditing] = useState(() => params.get("edit") === "1");
  const [discardEditor, setDiscardEditor] = useState(false);
  const options = useOwnApiKeyOptions();
  const update = useUpdateOwnApiKey(id);
  const revoke = useRevokeOwnApiKey();
  const remove = useDeleteOwnApiKey(id);
  const { t } = useI18n();
  const [submitting, setSubmitting] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [revokeReason, setRevokeReason] = useState("");
  const form = useForm<EditValues>({
    resolver: zodResolver(editSchema),
    defaultValues: emptyEditValues,
  });
  const { reset, formState: { isDirty } } = form;
  const loadVersion = useCallback((version: KeyVersion) => {
    const key = version.data;
    reset({
      name: key.name,
      status: key.status === "active" ? "active" : "disabled",
      expires_at: formatDateTimeLocalInput(key.expires_at),
      allowed_group_ids: key.allowed_group_ids,
      allowed_channel_ids: key.allowed_channel_ids,
      requests_per_minute: key.requests_per_minute,
      max_concurrent_requests: key.max_concurrent_requests,
      quota_limit_amount: key.quota_limit_amount,
    });
    setBaseline(version);
  }, [reset]);
  useEffect(() => {
    if (!error && data && data !== baseline && !isDirty && !conflict && !submitting) {
      loadVersion(data);
    }
  }, [data, error, baseline, isDirty, conflict, submitting, loadVersion]);

  const missing = error instanceof ApiError && error.isNotFound;
  const accessDenied = error instanceof ApiError && (error.isForbidden || error.isUnauthorized);
  const key = baseline?.data;
  const reloadRequired = conflict || (isDirty && !!error);
  const writeBlocked = submitting || reloadRequired || missing || accessDenied;
  const pageError = missing ? new Error(t("This API key is no longer available."))
    : key && reloadRequired && !accessDenied ? null : error;
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(
    submitting || (reloadRequired && isFetching) || revoke.isPending || remove.isPending, isDirty,
  );

  const reloadLatest = async () => {
    setDiscardOpen(false);
    const result = await refetch();
    if (result.isSuccess) {
      loadVersion(result.data);
      setConflict(false);
    } else if (result.error instanceof ApiError && result.error.isNotFound) {
      reset(emptyEditValues);
      setBaseline(undefined);
      setConflict(false);
    }
  };

  const onSubmit = async (values: EditValues) => {
    if (writeBlocked || !baseline || baseline.data.status === "revoked") return;
    setSubmitting(true);
    try {
      await update.mutateAsync({
        input: {
          name: values.name,
          status: values.status,
          expires_at: values.expires_at === formatDateTimeLocalInput(baseline.data.expires_at)
            ? baseline.data.expires_at
            : dateTimeLocalToIso(values.expires_at),
          allowed_group_ids: values.allowed_group_ids,
          allowed_channel_ids: values.allowed_channel_ids,
          requests_per_minute: values.requests_per_minute,
          max_concurrent_requests: values.max_concurrent_requests,
          quota_limit_amount: values.quota_limit_amount,
        },
        ifMatch: baseline.etag,
      });
      form.reset(values);
      markSaved();
      setEditing(false);
      toast.success(t("API key updated"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        setConflict(true);
      } else if (error instanceof ApiError && error.code === "api_key_target_not_allowed") {
        toast.error(t("One or more selected targets are no longer allowed by your API key policy."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Update failed"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const onInvalid = () => {
    toast.error(t("Review the highlighted API key fields."));
  };

  const confirmRevoke = async () => {
    setRevokeOpen(false);
    try {
      await revoke.mutateAsync({ id, reason: { reason: revokeReason || "revoked by owner" } });
      toast.success(t("API key revoked"));
      markSaved();
      navigate(returnTo, { replace: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Revoke failed"));
    }
  };

  const confirmDelete = async () => {
    setDeleteOpen(false);
    if (writeBlocked || !baseline) return;
    try {
      await remove.mutateAsync({ ifMatch: baseline.etag });
      toast.success(t("API key deleted"));
      markSaved();
      navigate(returnTo, { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This key was changed by another session. Reload before deleting it."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Delete failed"));
      }
    }
  };

  const targetGroups = useMemo<RoutingTargetGroup[]>(() => {
    const available = options.data?.groups ?? [];
    const missing = resourceLabels((key?.allowed_group_ids ?? [])
      .filter((groupId) => !available.some((group) => group.id === groupId))
      .map(id => ({id, name: t("Unavailable channel group")})));
    return [...available, ...Array.from(missing, ([id, name]) => ({id, name, enabled: false}))];
  }, [key?.allowed_group_ids, options.data?.groups, t]);
  const targetChannels = useMemo<RoutingTargetChannel[]>(() => {
    const available = options.data?.channels ?? [];
    const missing = resourceLabels((key?.allowed_channel_ids ?? [])
      .filter((channelId) =>
        !available.some((channel) => channel.id === channelId))
      .map(id => ({id, name: t("Unknown channel")})));
    return [...available, ...Array.from(missing, ([id, name]) => ({
        id, name,
        channel_group_id: "",
        channel_group_name: t("No longer allowed"),
        channel_group_enabled: false,
        enabled: false,
        auto_disabled: false,
      }))];
  }, [
    key?.allowed_channel_ids,
    options.data?.channels,
    t,
  ]);
  const selectedGroupIds = form.watch("allowed_group_ids");
  const selectedChannelIds = form.watch("allowed_channel_ids");
  const targetError =
    form.formState.errors.allowed_group_ids?.message ??
    form.formState.errors.allowed_channel_ids?.message;
  const allowedGroupNames = (key?.allowed_group_ids ?? []).map(
    (groupId) => targetGroups.find((group) => group.id === groupId)?.name ?? t("Unavailable channel group"),
  );
  const allowedChannelNames = (key?.allowed_channel_ids ?? []).map((channelId) => {
    const channel = targetChannels.find((channel) => channel.id === channelId);
    if (!channel) return t("Unknown channel");
    const groupName = channel.channel_group_name ?? targetGroups.find(group => group.id === channel.channel_group_id)?.name;
    return groupName ? `${channel.name} (${groupName})` : channel.name;
  });

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {navigationGuard}
      <PageHeader
        title={key && !missing ? key.name : t("API key")}
        backTo={returnTo}
        actions={key && !pageError && key.status !== "revoked" && <Button onClick={() => setEditing(true)}>{t("Edit")}</Button>}
      />
      <AsyncResource isLoading={isLoading} error={pageError}>
        {key && <>
            <section className="grid min-w-0 gap-4 border-y py-5 md:grid-cols-[minmax(12rem,1fr)_minmax(0,2fr)]">
              <header>
                <h2 className="text-sm font-semibold">{t("Details")}</h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  {t("Channel authorization covers all capabilities. Permissions are fixed at creation.")}
                </p>
              </header>
                <dl className="grid min-w-0 grid-cols-2 gap-4">
                  <DetailField
                    className="col-span-2"
                    label={t("API key")}
                    value={<ApiKeyValue value={key.secret} className="min-w-0 w-full" />}
                  />
                  <DetailField label={t("Status")} value={<StatusBadge value={key.status} />} />
                  <DetailField label={t("Expires")} value={formatExpiry(key.expires_at)} />
                  <DetailField label={t("Permissions")} value={formatList(key.permissions)} />
                  <DetailField
                    label={t("Channel groups")}
                    value={formatList(allowedGroupNames)}
                  />
                  <DetailField
                    label={t("Logical channels")}
                    value={formatList(allowedChannelNames)}
                  />
                  <DetailField
                    label={t("Requests / minute")}
                    value={key.requests_per_minute ?? "—"}
                  />
                  <DetailField
                    label={t("Max concurrent")}
                    value={key.max_concurrent_requests ?? "—"}
                  />
                  <DetailField
                    label={t("Quota limit")}
                    value={formatUsd(key.quota_limit_amount)}
                  />
                  <DetailField
                    label={t("Quota used")}
                    value={formatUsd(key.quota_used_amount)}
                  />
                  <DetailField label={t("Created")} value={formatDateTime(key.created_at)} />
                </dl>
            </section>
        {key.status !== "revoked" && <Sheet open={editing} onOpenChange={open => {
          if (submitting) return;
          if (!open && isDirty) setDiscardEditor(true); else setEditing(open);
        }}>
            <SheetContent className="overflow-y-auto data-[side=right]:w-full data-[side=right]:sm:max-w-2xl">
              <SheetHeader className="border-b pr-12">
                <SheetTitle>{t("Edit API key")}</SheetTitle>
                <SheetDescription>{t("Renaming or disabling takes effect immediately.")}</SheetDescription>
              </SheetHeader>
              <div className="flex flex-col gap-4 px-4 pb-4">
                {reloadRequired && (
                  <Alert variant="destructive">
                    <AlertTitle>{t("This version of the API key can no longer be saved.")}</AlertTitle>
                    <AlertDescription>
                      {error
                        ? t("The latest version could not be loaded. Your draft has been kept.")
                        : t("Load the latest version before editing again. This will discard your draft.")}
                    </AlertDescription>
                    <Button
                      type="button"
                      variant="outline"
                      className="justify-self-start"
                      disabled={isFetching}
                      onClick={() => isDirty ? setDiscardOpen(true) : void reloadLatest()}
                    >
                      {isFetching ? <Spinner data-icon="inline-start" /> : null}
                      {t("Load latest version")}
                    </Button>
                  </Alert>
                )}
                <form
                  onSubmit={form.handleSubmit(onSubmit, onInvalid)}
                  className="flex flex-col gap-4"
                >
                  <FieldSet disabled={writeBlocked} className="grid gap-5 xl:grid-cols-2">
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
                    <Controller
                      control={form.control}
                      name="status"
                      render={({ field, fieldState }) => (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor="status">{t("Status")}</FieldLabel>
                          <Select value={field.value} onValueChange={field.onChange}>
                            <SelectTrigger id="status" aria-invalid={fieldState.invalid}>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectGroup>
                                {API_KEY_STATUSES.filter((status) => status !== "revoked").map(
                                  (status) => (
                                    <SelectItem key={status} value={status}>
                                      {status === "active" ? t("Active") : t("Disabled")}
                                    </SelectItem>
                                  ),
                                )}
                              </SelectGroup>
                            </SelectContent>
                          </Select>
                          {fieldState.error ? (
                            <FieldError>{t(fieldState.error.message ?? "")}</FieldError>
                          ) : null}
                        </Field>
                      )}
                    />
                    <Field>
                      <FieldLabel htmlFor="expires_at">{t("Expires at (optional)")}</FieldLabel>
                      <Input
                        id="expires_at"
                        type="datetime-local"
                        {...form.register("expires_at")}
                      />
                    </Field>
                    {options.error ? (
                      <FieldError className="xl:col-span-2">
                        {options.error instanceof Error
                          ? options.error.message
                          : t("Unable to load API key target options.")}
                      </FieldError>
                    ) : null}
                    <div className="xl:col-span-2">
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
                  </FieldSet>
                  <Button type="submit" className="self-start" disabled={writeBlocked}>
                    {submitting ? <Spinner data-icon="inline-start" /> : null}
                    {t("Save changes")}
                  </Button>
                </form>
              </div>
            </SheetContent>
          </Sheet>}
          <section className="grid gap-4 border-b pb-5 md:grid-cols-[minmax(12rem,1fr)_minmax(0,2fr)]">
            <h2 className="text-sm font-semibold">{t("Danger zone")}</h2>
            <div className="flex min-w-0 flex-col gap-3">
                <p className="text-sm text-muted-foreground">
                  {t("Revocation keeps the Key visible. Deletion also erases its secret and hides it permanently.")}
                </p>
              <div className="flex flex-wrap gap-3">
                <Button
                  variant="destructive"
                  onClick={() => setRevokeOpen(true)}
                  disabled={
                    key.status === "revoked" ||
                    remove.isPending ||
                    revoke.isPending ||
                    writeBlocked
                  }
                >
                  {t("Revoke API key")}
                </Button>
                <Button
                  variant="destructive"
                  onClick={() => setDeleteOpen(true)}
                  disabled={remove.isPending || revoke.isPending || writeBlocked}
                >
                  {remove.isPending ? <Spinner data-icon="inline-start" /> : null}
                  {t("Delete API key")}
                </Button>
              </div>
            </div>
          </section>
        </>}
      </AsyncResource>
      <ConfirmDialog open={discardEditor} onOpenChange={setDiscardEditor} title={t("Discard unsaved changes?")}
        description={t("Your edits on this page will be lost.")} confirmLabel={t("Discard changes")} destructive
        onConfirm={() => { if (baseline) loadVersion(baseline); setDiscardEditor(false); setEditing(false); }} />

      <ConfirmDialog
        open={discardOpen}
        onOpenChange={setDiscardOpen}
        title={t("Discard unsaved changes?")}
        description={t("Load the latest version before editing again. This will discard your draft.")}
        confirmLabel={t("Discard changes")}
        destructive
        onConfirm={() => void reloadLatest()}
      />
      <ConfirmDialog
        open={revokeOpen}
        onOpenChange={(open) => {
          setRevokeOpen(open);
          if (!open) setRevokeReason("");
        }}
        title={t("Revoke API key?")}
        description={t("This permanently disables the key and records an audit entry.")}
        content={
          <Field>
            <FieldLabel htmlFor="revoke_reason">{t("Reason (optional)")}</FieldLabel>
            <Input
              id="revoke_reason"
              value={revokeReason}
              onChange={(event) => setRevokeReason(event.target.value)}
            />
          </Field>
        }
        confirmLabel={t("Revoke")}
        destructive
        onConfirm={confirmRevoke}
      />
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={t("Delete API key?")}
        description={t(
          "This revokes the Key, erases its stored secret, and removes it from the Console. Request logs and audit history keep the Key ID. This action cannot be undone.",
        )}
        confirmLabel={t("Delete API key")}
        destructive
        onConfirm={() => void confirmDelete()}
      />
    </div>
  );
}
