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
import { formatCurrency, formatList } from "@/lib/formatters";
import {
  dateTimeLocalToIso,
  formatDateTime,
  formatDateTimeLocalInput,
  formatExpiry,
} from "@/lib/dates";
import { API_KEY_STATUSES, apiFormatLabel } from "@/lib/permissions";

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
      toast.success("API key updated");
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error("This key was changed by another session. Reloading.");
      } else {
        toast.error(error instanceof Error ? error.message : "Update failed");
      }
    } finally {
      setSubmitting(false);
    }
  };

  const onInvalid = () => {
    toast.error("Review the highlighted API key fields.");
  };

  const confirmRevoke = async () => {
    setRevokeOpen(false);
    try {
      await revoke.mutateAsync({ id, reason: { reason: revokeReason || "revoked by owner" } });
      toast.success("API key revoked");
      navigate("/api-keys", { replace: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Revoke failed");
    }
  };

  const key = data?.data;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={key ? key.name : "API key"}
        description="View, rename, enable, disable, or revoke this key."
        actions={
          <Button variant="ghost" size="sm" onClick={() => navigate("/api-keys")}>
            <ArrowLeft data-icon="inline-start" /> Back
          </Button>
        }
      />
      <AsyncResource isLoading={isLoading} error={error}>
        {key ? (
          <>
            <Card>
              <CardHeader>
                <CardTitle>Details</CardTitle>
                <CardDescription>Authorization fields are managed by your policy.</CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                  <DetailField label="Status" value={<StatusBadge value={key.status} />} />
                  <DetailField label="Expires" value={formatExpiry(key.expires_at)} />
                  <DetailField
                    label="Formats"
                    value={formatList(key.allowed_api_formats.map(apiFormatLabel))}
                  />
                  <DetailField label="Permissions" value={formatList(key.permissions)} />
                  <DetailField label="Allowed groups" value={formatList(key.allowed_group_ids)} />
                  <DetailField label="Requests / minute" value={key.requests_per_minute ?? "—"} />
                  <DetailField
                    label="Max concurrent"
                    value={key.max_concurrent_requests ?? "—"}
                  />
                  <DetailField
                    label="Quota limit"
                    value={formatCurrency(key.quota_limit_amount)}
                  />
                  <DetailField
                    label="Quota used"
                    value={formatCurrency(key.quota_used_amount)}
                  />
                  <DetailField label="Created" value={formatDateTime(key.created_at)} />
                </dl>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>Edit</CardTitle>
                <CardDescription>Renaming or disabling takes effect immediately.</CardDescription>
              </CardHeader>
              <CardContent>
                <form
                  onSubmit={form.handleSubmit(onSubmit, onInvalid)}
                  className="flex flex-col gap-4"
                >
                  <FieldGroup>
                    <Field data-invalid={Boolean(form.formState.errors.name)}>
                      <FieldLabel htmlFor="name">Name</FieldLabel>
                      <Input
                        id="name"
                        aria-invalid={Boolean(form.formState.errors.name)}
                        {...form.register("name")}
                      />
                      {form.formState.errors.name ? (
                        <FieldError>{form.formState.errors.name.message}</FieldError>
                      ) : null}
                    </Field>
                    <Controller
                      control={form.control}
                      name="status"
                      defaultValue={formValues.status}
                      render={({ field, fieldState }) => (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor="status">Status</FieldLabel>
                          <Select value={field.value} onValueChange={field.onChange}>
                            <SelectTrigger id="status" aria-invalid={fieldState.invalid}>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectGroup>
                                {API_KEY_STATUSES.filter((status) => status !== "revoked").map(
                                  (status) => (
                                    <SelectItem key={status} value={status}>
                                      {status}
                                    </SelectItem>
                                  ),
                                )}
                              </SelectGroup>
                            </SelectContent>
                          </Select>
                          {fieldState.error ? (
                            <FieldError>{fieldState.error.message}</FieldError>
                          ) : null}
                        </Field>
                      )}
                    />
                    <Field>
                      <FieldLabel htmlFor="expires_at">Expires at (optional)</FieldLabel>
                      <Input
                        id="expires_at"
                        type="datetime-local"
                        {...form.register("expires_at")}
                      />
                    </Field>
                  </FieldGroup>
                  <Button type="submit" className="self-start" disabled={submitting}>
                    {submitting ? <Spinner data-icon="inline-start" /> : null}
                    Save changes
                  </Button>
                </form>
              </CardContent>
            </Card>

            <Separator />

            <Card>
              <CardHeader>
                <CardTitle className="text-destructive">Danger zone</CardTitle>
                <CardDescription>Revocation is permanent and audited.</CardDescription>
              </CardHeader>
              <CardContent>
                <Button
                  variant="destructive"
                  onClick={() => setRevokeOpen(true)}
                  disabled={key.status === "revoked"}
                >
                  Revoke API key
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
        title="Revoke API key?"
        description={
          <div className="flex flex-col gap-2">
            <span>This permanently disables the key and records an audit entry.</span>
            <Input
              placeholder="Reason (optional)"
              value={revokeReason}
              onChange={(event) => setRevokeReason(event.target.value)}
            />
          </div>
        }
        confirmLabel="Revoke"
        destructive
        onConfirm={confirmRevoke}
      />
    </div>
  );
}
