import { useState } from "react";
import { useParams, useNavigate } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ArrowLeft } from "lucide-react";
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
import { Separator } from "@/components/ui/separator";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import {
  useOwnApiKey,
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
import { API_KEY_STATUSES, apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const editSchema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  status: z.enum(["active", "disabled"]),
  expires_at: z.string().optional(),
});

type EditValues = z.infer<typeof editSchema>;

const emptyEditValues: EditValues = {
  name: "",
  status: "active",
  expires_at: "",
};

export function ApiKeyDetailPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useOwnApiKey(id);
  const update = useUpdateOwnApiKey(id);
  const revoke = useRevokeOwnApiKey();
  const { t } = useI18n();
  const [submitting, setSubmitting] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);
  const [revokeReason, setRevokeReason] = useState("");
  const formValues: EditValues = data
    ? {
        name: data.data.name,
        status: data.data.status === "active" ? "active" : "disabled",
        expires_at: formatDateTimeLocalInput(data.data.expires_at),
      }
    : emptyEditValues;

  const form = useForm<EditValues>({
    resolver: zodResolver(editSchema),
    defaultValues: emptyEditValues,
    values: formValues,
  });

  const onSubmit = async (values: EditValues) => {
    setSubmitting(true);
    try {
      await update.mutateAsync({
        input: {
          name: values.name,
          status: values.status,
          expires_at: dateTimeLocalToIso(values.expires_at),
        },
        ifMatch: etag,
      });
      toast.success(t("API key updated"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This key was changed by another session. Reloading."));
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
      navigate("/api-keys", { replace: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Revoke failed"));
    }
  };

  const key = data?.data;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={key ? key.name : t("API key")}
        description={t("View, rename, enable, disable, or revoke this key.")}
        actions={
          <Button variant="ghost" size="sm" onClick={() => navigate("/api-keys")}>
            <ArrowLeft data-icon="inline-start" /> {t("Back")}
          </Button>
        }
      />
      <AsyncResource isLoading={isLoading} error={error}>
        {key ? (
          <>
            <Card>
              <CardHeader>
                <CardTitle>{t("Details")}</CardTitle>
                <CardDescription>{t("Authorization fields are managed by your policy.")}</CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                  <DetailField label={t("Status")} value={<StatusBadge value={key.status} />} />
                  <DetailField label={t("Expires")} value={formatExpiry(key.expires_at)} />
                  <DetailField
                    label={t("Formats")}
                    value={formatList(key.allowed_api_formats.map(apiFormatLabel))}
                  />
                  <DetailField label={t("Permissions")} value={formatList(key.permissions)} />
                  <DetailField
                    label={t("Allowed groups")}
                    value={formatList(key.allowed_group_ids)}
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
                  </FieldGroup>
                  <Button type="submit" className="self-start" disabled={submitting}>
                    {submitting ? <Spinner data-icon="inline-start" /> : null}
                    {t("Save changes")}
                  </Button>
                </form>
              </CardContent>
            </Card>

            <Separator />

            <Card>
              <CardHeader>
                <CardTitle className="text-destructive">{t("Danger zone")}</CardTitle>
                <CardDescription>{t("Revocation is permanent and audited.")}</CardDescription>
              </CardHeader>
              <CardContent>
                <Button
                  variant="destructive"
                  onClick={() => setRevokeOpen(true)}
                  disabled={key.status === "revoked"}
                >
                  {t("Revoke API key")}
                </Button>
              </CardContent>
            </Card>
          </>
        ) : null}
      </AsyncResource>

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
    </div>
  );
}
