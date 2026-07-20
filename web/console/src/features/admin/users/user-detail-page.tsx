import { useState } from "react";
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
import { DecimalField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { useApiKeyPolicies, useUpdateUser, useUser } from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import { clearSession, setSession } from "@/api/session-store";
import { useSession } from "@/lib/use-session";
import { formatCurrency } from "@/lib/formatters";
import { formatDateTime } from "@/lib/dates";
import { ROLES, roleLabel, USER_STATUSES } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const editSchema = z.object({
  display_name: z.string().min(1, "Display name is required.").max(200),
  email: z.string().optional(),
  role: z.enum(["user", "admin"]),
  status: z.enum(["active", "suspended", "disabled"]),
  balance_amount: z.string().regex(/^-?\d+(?:\.\d+)?$/, "Enter a valid balance."),
  currency: z.string().min(1).max(8),
  default_api_key_policy_id: z.string().optional(),
});

type EditValues = z.infer<typeof editSchema>;

const emptyEditValues: EditValues = {
  display_name: "",
  email: "",
  role: "user",
  status: "active",
  balance_amount: "0",
  currency: "",
  default_api_key_policy_id: "",
};

export function UserDetailPage() {
  const { id = "" } = useParams();
  const { data, etag, isLoading, error } = useUser(id);
  const update = useUpdateUser(id);
  const policies = useApiKeyPolicies();
  const { user: currentUser } = useSession();
  const { t } = useI18n();
  const [submitting, setSubmitting] = useState(false);
  const formValues: EditValues = data
    ? {
        display_name: data.data.display_name,
        email: data.data.email ?? "",
        role: data.data.role,
        status:
          data.data.status === "active" || data.data.status === "suspended"
            ? data.data.status
            : "disabled",
        balance_amount: data.data.balance_amount,
        currency: data.data.currency,
        default_api_key_policy_id: data.data.default_api_key_policy_id ?? "",
      }
    : emptyEditValues;

  const form = useForm<EditValues>({
    resolver: zodResolver(editSchema),
    defaultValues: emptyEditValues,
    values: formValues,
  });

  const onSubmit = async (values: EditValues) => {
    const invalidatesCurrentSession =
      currentUser?.id === id &&
      (values.email !== (data?.data.email ?? "") ||
        values.role !== data?.data.role ||
        values.status !== data?.data.status);
    setSubmitting(true);
    try {
      await update.mutateAsync({
        input: {
          display_name: values.display_name,
          email: values.email || null,
          role: values.role,
          status: values.status,
          balance_amount: values.balance_amount,
          currency: values.currency,
          default_api_key_policy_id: values.default_api_key_policy_id || null,
        },
        ifMatch: etag,
      });
      if (currentUser?.id === id) {
        if (invalidatesCurrentSession) {
          toast.success(t("Account updated. Sign in again to continue."));
          clearSession();
          return;
        }
        setSession({
          user: {
            ...currentUser,
            display_name: values.display_name,
          },
        });
      }
      toast.success(t("User updated"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This user was changed elsewhere. Reloading."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Update failed"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const onInvalid = () => {
    toast.error(t("Review the highlighted account fields."));
  };

  const user = data?.data;

  return (
    <AdminDetailShell
      title={user?.display_name ?? t("User")}
      description={t("Manage a Console user's identity, role, and status.")}
      backPath="/admin/users"
      backLabel={t("Back to users")}
      isLoading={isLoading}
      error={error}
      hasData={Boolean(user)}
      detailCard={
        user ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Account")}</CardTitle>
              <CardDescription>{t("Read-only account facts.")}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField label={t("Email")} value={user.email ?? "—"} />
                <DetailField label={t("Role")} value={<StatusBadge value={user.role} />} />
                <DetailField label={t("Status")} value={<StatusBadge value={user.status} />} />
                <DetailField
                  label={t("Balance")}
                  value={formatCurrency(user.balance_amount, user.currency)}
                />
                <DetailField label={t("Created")} value={formatDateTime(user.created_at)} />
                <DetailField label={t("Updated")} value={formatDateTime(user.updated_at)} />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        user ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Edit user")}</CardTitle>
              <CardDescription>
                {t("Account, access, and balance changes take effect immediately.")}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form
                onSubmit={form.handleSubmit(onSubmit, onInvalid)}
                className="flex flex-col gap-4"
              >
                <FieldGroup>
                  <Field data-invalid={Boolean(form.formState.errors.display_name)}>
                    <FieldLabel htmlFor="display_name">{t("Display name")}</FieldLabel>
                    <Input
                      id="display_name"
                      aria-invalid={Boolean(form.formState.errors.display_name)}
                      {...form.register("display_name")}
                    />
                    {form.formState.errors.display_name ? (
                      <FieldError>{form.formState.errors.display_name.message}</FieldError>
                    ) : null}
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.email)}>
                    <FieldLabel htmlFor="email">{t("Email")}</FieldLabel>
                    <Input
                      id="email"
                      type="email"
                      aria-invalid={Boolean(form.formState.errors.email)}
                      {...form.register("email")}
                    />
                    {form.formState.errors.email ? (
                      <FieldError>{form.formState.errors.email.message}</FieldError>
                    ) : null}
                  </Field>
                  <Controller
                    control={form.control}
                    name="role"
                    defaultValue={formValues.role}
                    render={({ field, fieldState }) => (
                      <Field data-invalid={fieldState.invalid}>
                        <FieldLabel htmlFor="role">{t("Role")}</FieldLabel>
                        <Select value={field.value} onValueChange={field.onChange}>
                          <SelectTrigger id="role" aria-invalid={fieldState.invalid}>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectGroup>
                              {ROLES.map((role) => (
                                <SelectItem key={role} value={role}>
                                  {roleLabel(role)}
                                </SelectItem>
                              ))}
                            </SelectGroup>
                          </SelectContent>
                        </Select>
                        {fieldState.error ? (
                          <FieldError>{fieldState.error.message}</FieldError>
                        ) : null}
                      </Field>
                    )}
                  />
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
                              {USER_STATUSES.filter((status) => status !== "invited").map(
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
                  <DecimalField
                    id="balance_amount"
                    label={t("Balance")}
                    value={form.watch("balance_amount")}
                    onChange={(value) =>
                      form.setValue("balance_amount", value, {
                        shouldDirty: true,
                        shouldValidate: true,
                      })
                    }
                    error={form.formState.errors.balance_amount?.message}
                    required
                    description={t("Set the current account balance in the selected currency.")}
                  />
                  <Field data-invalid={Boolean(form.formState.errors.currency)}>
                    <FieldLabel htmlFor="currency">{t("Currency")}</FieldLabel>
                    <Input
                      id="currency"
                      aria-invalid={Boolean(form.formState.errors.currency)}
                      {...form.register("currency")}
                    />
                    {form.formState.errors.currency ? (
                      <FieldError>{form.formState.errors.currency.message}</FieldError>
                    ) : null}
                  </Field>
                  <Controller
                    control={form.control}
                    name="default_api_key_policy_id"
                    defaultValue={formValues.default_api_key_policy_id}
                    render={({ field, fieldState }) => (
                      <Field data-invalid={fieldState.invalid}>
                        <FieldLabel htmlFor="default_api_key_policy_id">
                          {t("Default API key policy")}
                        </FieldLabel>
                        <Select
                          value={field.value || "__none__"}
                          onValueChange={(value) =>
                            field.onChange(value === "__none__" ? "" : value)
                          }
                        >
                          <SelectTrigger
                            id="default_api_key_policy_id"
                            aria-invalid={fieldState.invalid}
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectGroup>
                              <SelectItem value="__none__">{t("None")}</SelectItem>
                              {policies.data
                                ?.filter((policy) => policy.enabled)
                                .map((policy) => (
                                  <SelectItem key={policy.id} value={policy.id}>
                                    {policy.name}
                                  </SelectItem>
                                ))}
                            </SelectGroup>
                          </SelectContent>
                        </Select>
                        {fieldState.error ? (
                          <FieldError>{fieldState.error.message}</FieldError>
                        ) : null}
                      </Field>
                    )}
                  />
                </FieldGroup>
                <Button type="submit" className="self-start" disabled={submitting}>
                  {submitting ? <Spinner data-icon="inline-start" /> : null}
                  {t("Save user")}
                </Button>
              </form>
            </CardContent>
          </Card>
        ) : null
      }
    />
  );
}
