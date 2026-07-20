import { useEffect, useState } from "react";
import { useParams } from "react-router";
import { useForm } from "react-hook-form";
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
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { useApiKeyPolicies, useUpdateUser, useUser } from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type { UserRole } from "@/api/types";
import { formatCurrency } from "@/lib/formatters";
import { formatDateTime } from "@/lib/dates";
import { ROLES, roleLabel, USER_STATUSES } from "@/lib/permissions";

const editSchema = z.object({
  display_name: z.string().min(1, "Display name is required.").max(200),
  email: z.string().optional(),
  role: z.enum(["user", "admin"]),
  status: z.enum(["active", "disabled"]),
  currency: z.string().min(1).max(8),
  default_api_key_policy_id: z.string().optional(),
});

type EditValues = z.infer<typeof editSchema>;

export function UserDetailPage() {
  const { id = "" } = useParams();
  const { data, etag, isLoading, error } = useUser(id);
  const update = useUpdateUser(id);
  const policies = useApiKeyPolicies();
  const [submitting, setSubmitting] = useState(false);

  const form = useForm<EditValues>({ resolver: zodResolver(editSchema) });

  useEffect(() => {
    if (data) {
      form.reset({
        display_name: data.data.display_name,
        email: data.data.email ?? "",
        role: data.data.role,
        status: (data.data.status === "active" ? "active" : "disabled") as "active" | "disabled",
        currency: data.data.currency,
        default_api_key_policy_id: data.data.default_api_key_policy_id ?? undefined,
      });
    }
  }, [data, form]);

  const onSubmit = async (values: EditValues) => {
    setSubmitting(true);
    try {
      await update.mutateAsync({
        input: {
          display_name: values.display_name,
          email: values.email || null,
          role: values.role,
          status: values.status,
          currency: values.currency,
          default_api_key_policy_id: values.default_api_key_policy_id || null,
        },
        ifMatch: etag,
      });
      toast.success("User updated");
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error("This user was changed elsewhere. Reloading.");
      } else {
        toast.error(error instanceof Error ? error.message : "Update failed");
      }
    } finally {
      setSubmitting(false);
    }
  };

  const user = data?.data;

  return (
    <AdminDetailShell
      title={user?.display_name ?? "User"}
      description="Manage a Console user's identity, role, and status."
      backPath="/admin/users"
      backLabel="Back to users"
      isLoading={isLoading}
      error={error}
      hasData={Boolean(user)}
      detailCard={
        user ? (
          <Card>
            <CardHeader>
              <CardTitle>Account</CardTitle>
              <CardDescription>Read-only account facts.</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField label="Email" value={user.email ?? "—"} />
                <DetailField label="Role" value={roleLabel(user.role)} />
                <DetailField label="Status" value={<StatusBadge value={user.status} />} />
                <DetailField
                  label="Balance"
                  value={formatCurrency(user.balance_amount, user.currency)}
                />
                <DetailField label="Created" value={formatDateTime(user.created_at)} />
                <DetailField label="Updated" value={formatDateTime(user.updated_at)} />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        user ? (
          <Card>
            <CardHeader>
              <CardTitle>Edit user</CardTitle>
              <CardDescription>Role and status changes take effect immediately.</CardDescription>
            </CardHeader>
            <CardContent>
              <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
                <FieldGroup>
                  <Field>
                    <FieldLabel htmlFor="display_name">Display name</FieldLabel>
                    <Input id="display_name" {...form.register("display_name")} />
                    {form.formState.errors.display_name ? (
                      <FieldError>{form.formState.errors.display_name.message}</FieldError>
                    ) : null}
                  </Field>
                  <Field>
                    <FieldLabel htmlFor="email">Email</FieldLabel>
                    <Input id="email" type="email" {...form.register("email")} />
                  </Field>
                  <Field>
                    <FieldLabel>Role</FieldLabel>
                    <Select
                      value={form.watch("role")}
                      onValueChange={(value) => form.setValue("role", value as UserRole)}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {ROLES.map((role) => (
                          <SelectItem key={role} value={role}>
                            {roleLabel(role)}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                  <Field>
                    <FieldLabel>Status</FieldLabel>
                    <Select
                      value={form.watch("status")}
                      onValueChange={(value) =>
                        form.setValue("status", value as "active" | "disabled")
                      }
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {USER_STATUSES.filter((status) => status !== "invited").map((status) => (
                          <SelectItem key={status} value={status}>
                            {status}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                  <Field>
                    <FieldLabel htmlFor="currency">Currency</FieldLabel>
                    <Input id="currency" {...form.register("currency")} />
                  </Field>
                  <Field>
                    <FieldLabel>Default API key policy</FieldLabel>
                    <Select
                      value={form.watch("default_api_key_policy_id") ?? "__none__"}
                      onValueChange={(value) =>
                        form.setValue(
                          "default_api_key_policy_id",
                          value === "__none__" ? undefined : value,
                        )
                      }
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="__none__">None</SelectItem>
                        {policies.data
                          ?.filter((policy) => policy.enabled)
                          .map((policy) => (
                            <SelectItem key={policy.id} value={policy.id}>
                              {policy.name}
                            </SelectItem>
                          ))}
                      </SelectContent>
                    </Select>
                  </Field>
                </FieldGroup>
                <Button type="submit" className="self-start" disabled={submitting}>
                  {submitting ? <Spinner data-icon="inline-start" /> : null}
                  Save user
                </Button>
              </form>
            </CardContent>
          </Card>
        ) : null
      }
    />
  );
}
