import { useMemo, useState } from "react";
import { useParams } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
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
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { useReturnPath } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";
import { ApiKeyValue } from "@/components/shared/api-key-value";
import { SharingChannelFields } from "@/components/shared/sharing-channel-fields";
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
import { formatList, formatUsd } from "@/lib/formatters";
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
  const returnTo = useReturnPath("/api-keys");
  const { data, etag, isLoading, error } = useOwnApiKey(id);
  const options = useOwnApiKeyOptions();
  const update = useUpdateOwnApiKey(id);
  const revoke = useRevokeOwnApiKey();
  const remove = useDeleteOwnApiKey(id);
  const { t } = useI18n();
  const [submitting, setSubmitting] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [revokeReason, setRevokeReason] = useState("");
  const formValues: EditValues = data
    ? {
        name: data.data.name,
        status: data.data.status === "active" ? "active" : "disabled",
        expires_at: formatDateTimeLocalInput(data.data.expires_at),
        allowed_group_ids: data.data.allowed_group_ids,
        allowed_channel_ids: data.data.allowed_channel_ids,
        requests_per_minute: data.data.requests_per_minute,
        max_concurrent_requests: data.data.max_concurrent_requests,
        quota_limit_amount: data.data.quota_limit_amount,
      }
    : emptyEditValues;

  const form = useForm<EditValues>({
    resolver: zodResolver(editSchema),
    defaultValues: emptyEditValues,
    values: formValues,
  });
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(
    submitting || revoke.isPending || remove.isPending, form.formState.isDirty,
  );

  const onSubmit = async (values: EditValues) => {
    setSubmitting(true);
    try {
      await update.mutateAsync({
        input: {
          name: values.name,
          status: values.status,
          expires_at: dateTimeLocalToIso(values.expires_at),
          allowed_group_ids: values.allowed_group_ids,
          allowed_channel_ids: values.allowed_channel_ids,
          requests_per_minute: values.requests_per_minute,
          max_concurrent_requests: values.max_concurrent_requests,
          quota_limit_amount: values.quota_limit_amount,
        },
        ifMatch: etag,
      });
      form.reset(values);
      markSaved();
      toast.success(t("API key updated"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This key was changed by another session. Reloading."));
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
    try {
      await remove.mutateAsync({ ifMatch: etag });
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

  const key = data?.data;
  const targetGroups = useMemo<RoutingTargetGroup[]>(() => {
    const available = options.data?.groups ?? [];
    const missing = (key?.allowed_group_ids ?? [])
      .filter((groupId) => !available.some((group) => group.id === groupId))
      .map((groupId) => ({
        id: groupId,
        name: groupId,
        enabled: false,
      }));
    return [...available, ...missing];
  }, [key?.allowed_group_ids, options.data?.groups]);
  const sharingChannelIds = useMemo(
    () => new Set(options.data?.sharing_channels.map((item) => item.channel_id) ?? []),
    [options.data?.sharing_channels],
  );
  const targetChannels = useMemo<RoutingTargetChannel[]>(() => {
    const available = options.data?.channels ?? [];
    const missing = (key?.allowed_channel_ids ?? [])
      .filter((channelId) =>
        !sharingChannelIds.has(channelId)
        && !available.some((channel) => channel.id === channelId))
      .map((channelId) => ({
        id: channelId,
        channel_group_id: "",
        channel_group_name: t("No longer allowed"),
        channel_group_enabled: false,
        name: channelId,
        enabled: false,
        auto_disabled: false,
      }));
    return [...available, ...missing];
  }, [
    key?.allowed_channel_ids,
    options.data?.channels,
    sharingChannelIds,
    t,
  ]);
  const selectedGroupIds = form.watch("allowed_group_ids");
  const selectedChannelIds = form.watch("allowed_channel_ids");
  const selectedSharingChannelIds = selectedChannelIds.filter((channelId) =>
    sharingChannelIds.has(channelId),
  );
  const selectedPolicyChannelIds = selectedChannelIds.filter(
    (channelId) => !sharingChannelIds.has(channelId),
  );
  const targetError =
    form.formState.errors.allowed_group_ids?.message ??
    form.formState.errors.allowed_channel_ids?.message;
  const allowedGroupNames = (key?.allowed_group_ids ?? []).map(
    (groupId) => targetGroups.find((group) => group.id === groupId)?.name ?? groupId,
  );
  const allowedChannelNames = [
    ...(options.data?.sharing_channels ?? [])
      .filter((channel) => key?.allowed_channel_ids.includes(channel.channel_id))
      .map((channel) => channel.channel_name),
    ...(key?.allowed_channel_ids ?? [])
      .filter((channelId) => !sharingChannelIds.has(channelId))
      .map((channelId) => {
        const channel = targetChannels.find((channel) => channel.id === channelId);
        return channel ? `${channel.name} (${channel.channel_group_name ?? channel.channel_group_id})` : channelId;
      }),
  ];

  return (
    <>
      <AdminDetailShell
        title={key ? key.name : t("API key")}
        description={t("View, rename, enable, disable, revoke, or delete this key.")}
        backPath={returnTo}
        isLoading={isLoading}
        error={error}
        hasData={Boolean(key)}
        navigationGuard={navigationGuard}
        detailCard={key ? (
            <Card>
              <CardHeader>
                <CardTitle>{t("Details")}</CardTitle>
                <CardDescription>
                  {t("Channel authorization covers all capabilities. Permissions are fixed at creation.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid min-w-0 grid-cols-1 gap-4">
                  <DetailField
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
              </CardContent>
            </Card>
        ) : null}
        editCard={key ? (
            <Card>
              <CardHeader>
                <CardTitle>{t("Edit")}</CardTitle>
                <CardDescription>{t("Renaming or disabling takes effect immediately.")}</CardDescription>
              </CardHeader>
              <CardContent>
                <form
                  onSubmit={form.handleSubmit(onSubmit, onInvalid)}
                  className="flex flex-col gap-4"
                >
                  <FieldGroup className="grid gap-5 xl:grid-cols-2">
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
                      defaultValue={formValues.status}
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
                    <div className="grid items-start gap-4 xl:col-span-2 xl:grid-cols-2">
                      <SharingChannelFields
                        channels={options.data?.sharing_channels ?? []}
                        selectedChannelIds={selectedSharingChannelIds}
                        onChange={(channelIds) =>
                          form.setValue(
                            "allowed_channel_ids",
                            [...selectedPolicyChannelIds, ...channelIds],
                            { shouldDirty: true, shouldValidate: true },
                          )
                        }
                      />
                      <RoutingTargetFields
                        groups={targetGroups}
                        channels={targetChannels}
                        selectedGroupIds={selectedGroupIds}
                        selectedChannelIds={selectedPolicyChannelIds}
                        onChange={(allowedGroupIds, allowedChannelIds) => {
                          form.setValue("allowed_group_ids", allowedGroupIds, {
                            shouldDirty: true,
                            shouldValidate: true,
                          });
                          form.setValue(
                            "allowed_channel_ids",
                            [...selectedSharingChannelIds, ...allowedChannelIds],
                            { shouldDirty: true, shouldValidate: true },
                          );
                        }}
                        legend={t("API Key Policy targets")}
                        description={t(options.data?.policy_enabled
                          ? "These ordinary groups and channels come from your enabled API Key Policy."
                          : "No enabled API Key Policy is assigned. Sharing credentials remain available.")}
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
                  <Button type="submit" className="self-start" disabled={submitting}>
                    {submitting ? <Spinner data-icon="inline-start" /> : null}
                    {t("Save changes")}
                  </Button>
                </form>
              </CardContent>
            </Card>
        ) : null}
        dangerZone={key ? (
          <div className="flex flex-col gap-3">
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
                    submitting
                  }
                >
                  {t("Revoke API key")}
                </Button>
                <Button
                  variant="destructive"
                  onClick={() => setDeleteOpen(true)}
                  disabled={remove.isPending || revoke.isPending || submitting}
                >
                  {remove.isPending ? <Spinner data-icon="inline-start" /> : null}
                  {t("Delete API key")}
                </Button>
              </div>
          </div>
        ) : null}
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
    </>
  );
}
