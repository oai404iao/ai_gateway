import { useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { SecretOnceDialog } from "@/components/shared/secret-once-dialog";
import { StatusBadge } from "@/components/shared/status-badge";
import { useApiKeyPolicies, useInviteUser, useUsers } from "@/features/admin/api";
import type { UserRole } from "@/api/types";
import { formatCurrency } from "@/lib/formatters";
import { formatRelative } from "@/lib/dates";
import { ROLES, roleLabel } from "@/lib/permissions";

const inviteSchema = z.object({
  email: z.string().email("Enter a valid email."),
  display_name: z.string().min(1, "Display name is required.").max(200),
  role: z.enum(["user", "admin"]),
  currency: z.string().min(1, "Currency is required.").max(8),
  default_api_key_policy_id: z.string().optional(),
});

type InviteValues = z.infer<typeof inviteSchema>;

export function UsersPage() {
  const { data, isLoading, error } = useUsers();
  const policies = useApiKeyPolicies();
  const invite = useInviteUser();
  const [open, setOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [token, setToken] = useState<string | null>(null);

  const form = useForm<InviteValues>({
    resolver: zodResolver(inviteSchema),
    defaultValues: { role: "user", currency: "USD" },
  });

  const onSubmit = async (values: InviteValues) => {
    setSubmitting(true);
    try {
      const result = await invite.mutateAsync({
        email: values.email,
        display_name: values.display_name,
        role: values.role,
        currency: values.currency,
        default_api_key_policy_id: values.default_api_key_policy_id || null,
      });
      setToken(result.invitation_token);
      setOpen(false);
      form.reset({ role: "user", currency: "USD" });
      toast.success("Invitation issued");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Invite failed");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <>
      <AdminListPage
        title="Users"
        description="Console users, roles, and balances. New users join by invitation."
        query={{ data, isLoading, error }}
        rowKey={(user) => user.id}
        detailPath={(user) => `/admin/users/${user.id}`}
        createLabel="Invite user"
        onCreate={() => setOpen(true)}
        columns={[
          {
            key: "name",
            header: "Name",
            render: (user) => (
              <span className="flex flex-col">
                <span className="font-medium">{user.display_name}</span>
                <span className="text-xs text-muted-foreground">{user.email ?? "—"}</span>
              </span>
            ),
          },
          { key: "role", header: "Role", render: (user) => roleLabel(user.role) },
          { key: "status", header: "Status", render: (user) => <StatusBadge value={user.status} /> },
          {
            key: "balance",
            header: "Balance",
            render: (user) => formatCurrency(user.balance_amount, user.currency),
          },
          { key: "updated", header: "Updated", render: (user) => formatRelative(user.updated_at) },
        ]}
      />

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Invite user</DialogTitle>
            <DialogDescription>
              The invitation token is shown once and must be delivered out of band.
            </DialogDescription>
          </DialogHeader>
          <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="email">Email</FieldLabel>
                <Input id="email" type="email" {...form.register("email")} />
                {form.formState.errors.email ? (
                  <FieldError>{form.formState.errors.email.message}</FieldError>
                ) : null}
              </Field>
              <Field>
                <FieldLabel htmlFor="display_name">Display name</FieldLabel>
                <Input id="display_name" {...form.register("display_name")} />
                {form.formState.errors.display_name ? (
                  <FieldError>{form.formState.errors.display_name.message}</FieldError>
                ) : null}
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
                <FieldLabel htmlFor="currency">Currency</FieldLabel>
                <Input id="currency" {...form.register("currency")} />
                {form.formState.errors.currency ? (
                  <FieldError>{form.formState.errors.currency.message}</FieldError>
                ) : null}
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
            <DialogFooter>
              <Button type="submit" disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                Send invitation
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <SecretOnceDialog
        open={Boolean(token)}
        onOpenChange={(open) => !open && setToken(null)}
        title="Invitation token"
        description="Give this to the new user to activate their account."
        secret={token}
      />
    </>
  );
}
