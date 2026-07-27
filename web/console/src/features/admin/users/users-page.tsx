import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { CheckCheck, ListChecks, Plus } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
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
import { AsyncResource } from "@/components/shared/async-resource";
import { DecimalField } from "@/components/shared/decimal-field";
import { PageHeader } from "@/components/shared/page-header";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { SecretOnceDialog } from "@/components/shared/secret-once-dialog";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useApiKeyPolicies,
  useInviteUser,
  useUserGroups,
  useUsers,
} from "@/features/admin/api";
import { UserBatchEditDialog } from "@/features/admin/users/user-batch-edit-dialog";
import { formatUsd } from "@/lib/formatters";
import { formatRelative } from "@/lib/dates";
import { ROLES, roleLabel } from "@/lib/permissions";
import { useSession } from "@/lib/use-session";
import { useI18n } from "@/app/i18n";
import type { ControlPlaneUser, UserRole } from "@/api/types";

export function UsersPage() {
  const navigate = useNavigate();
  const users = useUsers();
  const groups = useUserGroups();
  const policies = useApiKeyPolicies();
  const invite = useInviteUser();
  const { user: currentUser } = useSession();
  const { t } = useI18n();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [batchOpen, setBatchOpen] = useState(false);
  const [inviteOpen, setInviteOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [token, setToken] = useState<string | null>(null);
  const batchDialogTriggerId = "user-batch-edit-trigger";

  const inviteSchema = z.object({
    email: z.string().email(t("Enter a valid email.")),
    display_name: z.string().min(1, t("Display name is required.")).max(200),
    role: z.enum(["user", "admin"]),
    user_group_id: z.string().min(1, t("Pick a user group.")),
    initial_balance_amount: z
      .string()
      .regex(/^\d+(?:\.\d+)?$/, t("Enter a valid non-negative balance.")),
    default_api_key_policy_id: z.string().optional(),
  });
  type InviteValues = z.infer<typeof inviteSchema>;

  const form = useForm<InviteValues>({
    resolver: zodResolver(inviteSchema),
    defaultValues: {
      email: "",
      display_name: "",
      role: "user",
      user_group_id: "",
      initial_balance_amount: "0",
      default_api_key_policy_id: "",
    },
  });

  useEffect(() => {
    const available = new Set((users.data ?? []).map((user) => user.id));
    setSelected((current) => {
      const next = new Set([...current].filter((id) => available.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [users.data]);

  useEffect(() => {
    if (form.getValues("user_group_id")) return;
    const defaultGroup = groups.data?.find(
      (group) => group.system_role === form.getValues("role"),
    );
    if (defaultGroup) {
      form.setValue("user_group_id", defaultGroup.id, {
        shouldValidate: true,
      });
    }
  }, [form, groups.data]);

  const selectedUsers = useMemo(
    () => (users.data ?? []).filter((user) => selected.has(user.id)),
    [selected, users.data],
  );
  const allSelected =
    (users.data?.length ?? 0) > 0 &&
    selectedUsers.length === users.data?.length;
  const groupName = (id: string) =>
    groups.data?.find((group) => group.id === id)?.name ?? id;
  const policyName = (id: string | null) =>
    id ? policies.data?.find((policy) => policy.id === id)?.name ?? id : t("None");

  const toggleUser = (user: ControlPlaneUser) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(user.id)) next.delete(user.id);
      else next.add(user.id);
      return next;
    });
  };

  const onRoleChange = (
    role: UserRole,
    onChange: (value: UserRole) => void,
  ) => {
    const currentGroupId = form.getValues("user_group_id");
    const currentGroup = groups.data?.find((group) => group.id === currentGroupId);
    onChange(role);
    if (!currentGroup || currentGroup.system_role) {
      const defaultGroup = groups.data?.find(
        (group) => group.system_role === role,
      );
      form.setValue("user_group_id", defaultGroup?.id ?? "", {
        shouldDirty: true,
        shouldValidate: true,
      });
    }
  };

  const onSubmit = async (values: InviteValues) => {
    setSubmitting(true);
    try {
      const result = await invite.mutateAsync({
        email: values.email,
        display_name: values.display_name,
        role: values.role,
        user_group_id: values.user_group_id,
        initial_balance_amount: values.initial_balance_amount,
        default_api_key_policy_id: values.default_api_key_policy_id || null,
      });
      setToken(result.invitation_token);
      setInviteOpen(false);
      const defaultGroup = groups.data?.find(
        (group) => group.system_role === "user",
      );
      form.reset({
        email: "",
        display_name: "",
        role: "user",
        user_group_id: defaultGroup?.id ?? "",
        initial_balance_amount: "0",
        default_api_key_policy_id: "",
      });
      toast.success(t("Invitation issued"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Invite failed"));
    } finally {
      setSubmitting(false);
    }
  };

  const columns: Column<ControlPlaneUser>[] = [
    {
      key: "selected",
      header: t("Select"),
      render: (user) => (
        <Checkbox
          aria-label={`${t("Select")} ${user.display_name}`}
          checked={selected.has(user.id)}
          onCheckedChange={() => toggleUser(user)}
        />
      ),
      className: "w-12",
    },
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
      key: "group",
      header: t("User group"),
      render: (user) => groupName(user.user_group_id),
    },
    {
      key: "status",
      header: t("Status"),
      render: (user) => <StatusBadge value={user.status} />,
    },
    {
      key: "balance",
      header: t("Balance"),
      render: (user) => formatUsd(user.balance_amount),
    },
    {
      key: "policy",
      header: t("Effective API policy"),
      render: (user) => policyName(user.effective_api_key_policy_id),
    },
    {
      key: "updated",
      header: t("Updated"),
      render: (user) => formatRelative(user.updated_at),
    },
  ];

  return (
    <>
      <div className="flex flex-col gap-6">
        <PageHeader
          title={t("Users")}
          description={t(
            "Console users, groups, policies, and balances. New users join by invitation.",
          )}
          actions={
            <>
              <Button
                variant="outline"
                disabled={(users.data?.length ?? 0) === 0}
                onClick={() => {
                  if (allSelected) setSelected(new Set());
                  else {
                    setSelected(
                      new Set((users.data ?? []).map((user) => user.id)),
                    );
                  }
                }}
              >
                <CheckCheck data-icon="inline-start" />
                {allSelected ? t("Clear selection") : t("Select all")}
              </Button>
              <Button
                id={batchDialogTriggerId}
                variant="outline"
                disabled={selectedUsers.length === 0}
                onClick={() => setBatchOpen(true)}
              >
                <ListChecks data-icon="inline-start" />
                {t("Batch edit ({count})", { count: selectedUsers.length })}
              </Button>
              <Button onClick={() => setInviteOpen(true)}>
                <Plus data-icon="inline-start" />
                {t("Invite user")}
              </Button>
            </>
          }
        />
        <Card>
          <CardHeader>
            <CardTitle>{t("Users")}</CardTitle>
            <CardDescription>{t("Click a row to view or edit.")}</CardDescription>
          </CardHeader>
          <CardContent>
            <AsyncResource
              isLoading={users.isLoading || groups.isLoading || policies.isLoading}
              error={users.error ?? groups.error ?? policies.error}
              isEmpty={users.data?.length === 0}
              emptyTitle={t("No records")}
              emptyDescription={t("There are no records to show yet.")}
            >
              <ResourceTable
                columns={columns}
                rows={users.data ?? []}
                rowKey={(user) => user.id}
                onRowClick={(user) => navigate(`/admin/users/${user.id}`)}
              />
            </AsyncResource>
          </CardContent>
        </Card>
      </div>

      <Dialog open={inviteOpen} onOpenChange={setInviteOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("Invite user")}</DialogTitle>
            <DialogDescription>
              {t("The invitation token is shown once and must be delivered out of band.")}
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={form.handleSubmit(onSubmit, () =>
              toast.error(t("Review the highlighted invitation fields.")),
            )}
            className="flex flex-col gap-4"
          >
            <FieldGroup>
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
              <Controller
                control={form.control}
                name="role"
                defaultValue="user"
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor="invite_role">{t("Role")}</FieldLabel>
                    <Select
                      value={field.value}
                      onValueChange={(value) =>
                        onRoleChange(value as UserRole, field.onChange)
                      }
                    >
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
              <Controller
                control={form.control}
                name="user_group_id"
                defaultValue=""
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor="invite_user_group_id">
                      {t("User group")}
                    </FieldLabel>
                    <Select value={field.value} onValueChange={field.onChange}>
                      <SelectTrigger
                        id="invite_user_group_id"
                        aria-invalid={fieldState.invalid}
                      >
                        <SelectValue placeholder={t("Pick a user group")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          {groups.data?.map((group) => (
                            <SelectItem key={group.id} value={group.id}>
                              {group.name}
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
                name="default_api_key_policy_id"
                defaultValue=""
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor="invite_default_api_key_policy_id">
                      {t("API policy override")}
                    </FieldLabel>
                    <Select
                      value={field.value || "__inherit__"}
                      onValueChange={(value) =>
                        field.onChange(value === "__inherit__" ? "" : value)
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
                          <SelectItem value="__inherit__">
                            {t("Inherit group policy")}
                          </SelectItem>
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
              <DecimalField
                id="invite_initial_balance_amount"
                label={t("Initial balance")}
                value={form.watch("initial_balance_amount")}
                onChange={(value) =>
                  form.setValue("initial_balance_amount", value, {
                    shouldDirty: true,
                    shouldValidate: true,
                  })
                }
                error={form.formState.errors.initial_balance_amount?.message}
                required
                description={t("Starting USD credit available after activation.")}
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

      <UserBatchEditDialog
        open={batchOpen}
        users={selectedUsers}
        groups={groups.data ?? []}
        policies={policies.data ?? []}
        currentUserId={currentUser?.id}
        onOpenChange={setBatchOpen}
        onApplied={() => setSelected(new Set())}
        triggerId={batchDialogTriggerId}
      />

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
