import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
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
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { SecretOnceDialog } from "@/components/shared/secret-once-dialog";
import { StatusBadge } from "@/components/shared/status-badge";
import { useApiKeyPolicies, useInviteUser, useUsers } from "@/features/admin/api";
import { formatCurrency } from "@/lib/formatters";
import { formatRelative } from "@/lib/dates";
import { ROLES, roleLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export function UsersPage() {
  const { data, isLoading, error } = useUsers();
  const policies = useApiKeyPolicies();
  const invite = useInviteUser();
  const [open, setOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [token, setToken] = useState<string | null>(null);
  const { t } = useI18n();
  const inviteSchema = z.object({
    email: z.string().email(t("Enter a valid email.")),
    display_name: z.string().min(1, t("Display name is required.")).max(200),
    role: z.enum(["user", "admin"]),
    currency: z.string().min(1, t("Currency is required.")).max(8),
    default_api_key_policy_id: z.string().optional(),
  });
  type InviteValues = z.infer<typeof inviteSchema>;

  const form = useForm<InviteValues>({
    resolver: zodResolver(inviteSchema),
    defaultValues: {
      email: "",
      display_name: "",
      role: "user",
      currency: "USD",
      default_api_key_policy_id: "",
    },
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
      form.reset({
        email: "",
        display_name: "",
        role: "user",
        currency: "USD",
        default_api_key_policy_id: "",
      });
      toast.success(t("Invitation issued"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Invite failed"));
    } finally {
      setSubmitting(false);
    }
  };

  const onInvalid = () => {
    toast.error(t("Review the highlighted invitation fields."));
  };

  return (
    <>
      <AdminListPage
        title={t("Users")}
        description={t("Console users, roles, and balances. New users join by invitation.")}
        query={{ data, isLoading, error }}
        rowKey={(user) => user.id}
        detailPath={(user) => `/admin/users/${user.id}`}
        createLabel={t("Invite user")}
        onCreate={() => setOpen(true)}
        columns={[
          {
            key: "name",
            header: t("Name"),
            render: (user) => (
              <span className="flex flex-col">
                <span className="font-medium">{user.display_name}</span>
                <span className="text-xs text-muted-foreground">{user.email ?? "—"}</span>
              </span>
            ),
          },
          {
            key: "role",
            header: t("Role"),
            render: (user) => <StatusBadge value={user.role} />,
          },
          {
            key: "status",
            header: t("Status"),
            render: (user) => <StatusBadge value={user.status} />,
          },
          {
            key: "balance",
            header: t("Balance"),
            render: (user) => formatCurrency(user.balance_amount, user.currency),
          },
          {
            key: "updated",
            header: t("Updated"),
            render: (user) => formatRelative(user.updated_at),
          },
        ]}
      />

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("Invite user")}</DialogTitle>
            <DialogDescription>
              {t("The invitation token is shown once and must be delivered out of band.")}
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={form.handleSubmit(onSubmit, onInvalid)}
            className="flex flex-col gap-4"
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="email">{t("Email")}</FieldLabel>
                <Input id="email" type="email" {...form.register("email")} />
                {form.formState.errors.email ? (
                  <FieldError>{form.formState.errors.email.message}</FieldError>
                ) : null}
              </Field>
              <Field>
                <FieldLabel htmlFor="display_name">{t("Display name")}</FieldLabel>
                <Input id="display_name" {...form.register("display_name")} />
                {form.formState.errors.display_name ? (
                  <FieldError>{form.formState.errors.display_name.message}</FieldError>
                ) : null}
              </Field>
              <Controller
                control={form.control}
                name="role"
                defaultValue="user"
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor="invite_role">{t("Role")}</FieldLabel>
                    <Select value={field.value} onValueChange={field.onChange}>
                      <SelectTrigger id="invite_role" aria-invalid={fieldState.invalid}>
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
              <Field>
                <FieldLabel htmlFor="currency">{t("Currency")}</FieldLabel>
                <Input id="currency" {...form.register("currency")} />
                {form.formState.errors.currency ? (
                  <FieldError>{form.formState.errors.currency.message}</FieldError>
                ) : null}
              </Field>
              <Controller
                control={form.control}
                name="default_api_key_policy_id"
                defaultValue=""
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor="invite_default_api_key_policy_id">
                      {t("Default API key policy")}
                    </FieldLabel>
                    <Select
                      value={field.value || "__none__"}
                      onValueChange={(value) =>
                        field.onChange(value === "__none__" ? "" : value)
                      }
                    >
                      <SelectTrigger
                        id="invite_default_api_key_policy_id"
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
            <DialogFooter>
              <Button type="submit" disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {t("Send invitation")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <SecretOnceDialog
        open={Boolean(token)}
        onOpenChange={(open) => !open && setToken(null)}
        title={t("Invitation token")}
        description={t("Give this to the new user to activate their account.")}
        secret={token}
      />
    </>
  );
}
