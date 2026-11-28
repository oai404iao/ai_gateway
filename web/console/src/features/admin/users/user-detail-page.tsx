import { useEffect, useRef, useState } from "react";
import { useParams } from "react-router";
import { useReturnPath } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/hooks/use-configuration-draft";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { toast } from "sonner";
import type { TemporaryPasswordResponse, UserUpdateInput } from "@/api/types";
import { ApiError } from "@/api/errors";
import { clearSession, getSession, setSession, sessionScope } from "@/api/session-store";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
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
import { Switch } from "@/components/ui/switch";
import { DecimalField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import {
  useApiKeyPolicies,
  useDeleteUser,
  useIssueTemporaryPassword,
  useReissueUserInvitation,
  useUpdateUser,
  useUserGroups,
  useUser,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { formatDateTime } from "@/lib/dates";
import { formatUsd } from "@/lib/formatters";
import {
  ROLES,
  roleLabel,
  userStatusLabel,
} from "@/lib/permissions";
import { useSession } from "@/lib/use-session";

import { UserDetailDialogs, UserPasswordRecoveryCard } from "./user-detail-dialogs";
import {
  accountSchema,
  balanceSchema,
  emptyAccountValues,
  emptyBalanceValues,
  emptyStatusValues,
  isManageableStatus,
  manageableStatuses,
  statusSchema,
  type AccountValues,
  type BalanceValues,
  type StatusValues,
  type SubmittingAction,
} from "./user-detail-schema";
export function UserDetailPage() {
  const { id = "" } = useParams();
  const returnTo = useReturnPath("/admin/users");
  const { data, etag, isLoading, error, refetch } = useUser(id);
  const update = useUpdateUser(id);
  const remove = useDeleteUser(id);
  const reissue = useReissueUserInvitation(id);
  const issueTemporaryPassword = useIssueTemporaryPassword(id);
  const policies = useApiKeyPolicies();
  const groups = useUserGroups();
  const { user: currentUser } = useSession();
  const { t } = useI18n();
  const [submitting, setSubmitting] = useState<SubmittingAction | null>(null);
  const [reissueOpen, setReissueOpen] = useState(false);
  const [temporaryPasswordOpen, setTemporaryPasswordOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [invitationToken, setInvitationToken] = useState<string | null>(null);
  const [adminPassword, setAdminPassword] = useState("");
  const [websocketEnabled, setWebsocketEnabled] = useState(false);
  const [websocketDirty, setWebsocketDirty] = useState(false);
  const websocketBaselineRef = useRef<typeof data>(undefined);
  const [temporaryPassword, setTemporaryPassword] =
    useState<TemporaryPasswordResponse | null>(null);
  const user = data?.data;

  useEffect(() => {
    websocketBaselineRef.current = undefined;
    setWebsocketDirty(false);
  }, [id]);

  useEffect(() => {
    if (data && data !== websocketBaselineRef.current && !websocketDirty) {
      websocketBaselineRef.current = data;
      setWebsocketEnabled(data.data.websocket_enabled);
    }
  }, [data, websocketDirty]);

  const accountValues: AccountValues = user
    ? {
        display_name: user.display_name,
        email: user.email ?? "",
        role: user.role,
        user_group_id: user.user_group_id,
        default_api_key_policy_id: user.default_api_key_policy_id ?? "",
      }
    : emptyAccountValues;
  const balanceValues: BalanceValues = user
    ? { balance_amount: user.balance_amount }
    : emptyBalanceValues;
  const statusValues: StatusValues =
    user && isManageableStatus(user.status) ? { status: user.status } : emptyStatusValues;

  const accountForm = useForm<AccountValues>({
    resolver: zodResolver(accountSchema),
    defaultValues: emptyAccountValues,
    values: accountValues,
    resetOptions: { keepDirtyValues: true },
  });
  const balanceForm = useForm<BalanceValues>({
    resolver: zodResolver(balanceSchema),
    defaultValues: emptyBalanceValues,
    values: balanceValues,
    resetOptions: { keepDirtyValues: true },
  });
  const statusForm = useForm<StatusValues>({
    resolver: zodResolver(statusSchema),
    defaultValues: emptyStatusValues,
    values: statusValues,
    resetOptions: { keepDirtyValues: true },
  });
  const draft = useConfigurationDraft(
    submitting !== null || remove.isPending,
    accountForm.formState.isDirty || balanceForm.formState.isDirty ||
      statusForm.formState.isDirty || Boolean(user && websocketEnabled !== user.websocket_enabled),
  );

  const applyUpdate = async ({
    action,
    input,
    successMessage,
    invalidatesCurrentSession = false,
    updatedDisplayName,
  }: {
    action: SubmittingAction;
    input: UserUpdateInput;
    successMessage: string;
    invalidatesCurrentSession?: boolean;
    updatedDisplayName?: string;
  }) => {
    setSubmitting(action);
    try {
      await update.mutateAsync({ input, ifMatch: etag });
      if (action === "account") {
        accountForm.reset(accountForm.getValues());
      } else if (action === "balance") {
        balanceForm.reset(balanceForm.getValues());
      } else if (action === "status") {
        statusForm.reset(statusForm.getValues());
      } else if (action === "settings") {
        setWebsocketDirty(false);
      }
      const activeUser = getSession().user;
      if (activeUser && currentUser?.id === id && sessionScope(activeUser) === sessionScope(currentUser)) {
        if (invalidatesCurrentSession) {
          draft.markSaved();
          toast.success(t("Account updated. Sign in again to continue."));
          clearSession();
          return;
        }
        if (updatedDisplayName !== undefined) {
          setSession({
            user: {
              ...activeUser,
              display_name: updatedDisplayName,
            },
          });
        }
      }
      toast.success(t(successMessage));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This user was changed elsewhere. Reloading."));
        await refetch();
      } else {
        toast.error(error instanceof Error ? error.message : t("Update failed"));
      }
    } finally {
      setSubmitting(null);
    }
  };

  const submitAccount = async (values: AccountValues) => {
    if (!user) return;
    const input: UserUpdateInput = {};
    if (values.display_name !== user.display_name) {
      input.display_name = values.display_name;
    }
    if (values.email !== (user.email ?? "")) {
      input.email = values.email || null;
    }
    if (values.role !== user.role) {
      input.role = values.role;
    }
    if (values.user_group_id !== user.user_group_id) {
      input.user_group_id = values.user_group_id;
    }
    if (values.default_api_key_policy_id !== (user.default_api_key_policy_id ?? "")) {
      input.default_api_key_policy_id = values.default_api_key_policy_id || null;
    }
    if (Object.keys(input).length === 0) {
      toast.info(t("No account changes to save."));
      return;
    }
    await applyUpdate({
      action: "account",
      input,
      successMessage: "Account details updated",
      invalidatesCurrentSession: input.email !== undefined || input.role !== undefined,
      updatedDisplayName: input.display_name,
    });
  };

  const submitBalance = async (values: BalanceValues) => {
    if (!user) return;
    if (values.balance_amount === user.balance_amount) {
      toast.info(t("Balance is unchanged."));
      return;
    }
    await applyUpdate({
      action: "balance",
      input: { balance_amount: values.balance_amount },
      successMessage: "Balance updated",
    });
  };

  const submitStatus = async (values: StatusValues) => {
    if (!user || !isManageableStatus(user.status)) return;
    if (values.status === user.status) {
      toast.info(t("Status is unchanged."));
      return;
    }
    await applyUpdate({
      action: "status",
      input: { status: values.status },
      successMessage: "Access status updated",
      invalidatesCurrentSession: true,
    });
  };

  const savePersonalSettings = async () => {
    if (!user || websocketEnabled === user.websocket_enabled) return;
    await applyUpdate({
      action: "settings",
      input: { websocket_enabled: websocketEnabled },
      successMessage: "Personal settings updated",
    });
  };

  const onInvalidAccount = () => {
    toast.error(t("Review the highlighted account fields."));
  };
  const onInvalidBalance = () => {
    toast.error(t("Review the highlighted balance field."));
  };

  const reissueInvitation = async () => {
    setReissueOpen(false);
    try {
      const invitation = await reissue.mutateAsync();
      setInvitationToken(invitation.invitation_token);
      toast.success(t("Replacement invitation issued"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Invitation recovery failed"));
    }
  };

  const generateTemporaryPassword = async () => {
    setTemporaryPasswordOpen(false);
    try {
      const issued = await issueTemporaryPassword.mutateAsync({
        current_password: adminPassword,
      });
      setTemporaryPassword(issued);
      issueTemporaryPassword.reset();
      toast.success(t("Temporary password generated"));
    } catch (error) {
      if (error instanceof ApiError && error.code === "reauthentication_failed") {
        toast.error(t("Your administrator password is incorrect."));
      } else if (
        error instanceof ApiError &&
        error.code === "temporary_password_unavailable"
      ) {
        toast.error(t("This account cannot receive a temporary password."));
      } else {
        toast.error(
          error instanceof Error
            ? error.message
            : t("Temporary password generation failed"),
        );
      }
    } finally {
      setAdminPassword("");
    }
  };

  const deleteUser = async () => {
    setDeleteOpen(false);
    try {
      await remove.mutateAsync({ ifMatch: etag });
      toast.success(t("User deleted"));
      draft.markSaved();
      draft.navigate(returnTo, { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.code === "cannot_delete_self") {
        toast.error(t("You cannot delete your own administrator account."));
      } else if (
        error instanceof ApiError &&
        error.code === "last_administrator"
      ) {
        toast.error(t("Create another active administrator before deleting this user."));
      } else if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This user was changed elsewhere. Reloading."));
        await refetch();
      } else {
        toast.error(error instanceof Error ? error.message : t("Delete failed"));
      }
    }
  };

  const groupName =
    groups.data?.find((group) => group.id === user?.user_group_id)?.name ??
    t("Unavailable");
  const effectivePolicyName = user?.effective_api_key_policy_id
    ? policies.data?.find(
        (policy) => policy.id === user.effective_api_key_policy_id,
      )?.name ?? t("Unavailable")
    : t("None");

  return (
    <>
      <AdminDetailShell
      title={user?.display_name ?? t("User")}
      backPath={returnTo}
      navigationGuard={draft.navigationGuard}
      backLabel={t("Back to users")}
      isLoading={isLoading || policies.isLoading || groups.isLoading}
      error={error ?? policies.error ?? groups.error}
      hasData={Boolean(user)}
      detailCard={
        user ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Account")}</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4">
                <DetailField label={t("Email")} value={user.email ?? "—"} />
                <DetailField label={t("Role")} value={<StatusBadge value={user.role} />} />
                <DetailField label={t("Status")} value={<StatusBadge value={user.status} />} />
                <DetailField label={t("User group")} value={groupName} />
                <DetailField
                  label={t("Effective API policy")}
                  value={effectivePolicyName}
                />
                <DetailField
                  label={t("Responses WebSocket")}
                  value={t(user.websocket_enabled ? "Enabled" : "Disabled")}
                />
                <DetailField label={t("Balance")} value={formatUsd(user.balance_amount)} />
                <DetailField label={t("Created")} value={formatDateTime(user.created_at)} />
                <DetailField label={t("Updated")} value={formatDateTime(user.updated_at)} />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        user ? (
          <>
            <Card>
              <CardHeader>
                <CardTitle>{t("Account details")}</CardTitle>
              </CardHeader>
              <CardContent>
                <form
                  onSubmit={accountForm.handleSubmit(submitAccount, onInvalidAccount)}
                  className="flex flex-col gap-4"
                >
                  <FieldGroup className="grid gap-5 xl:grid-cols-2">
                    <Field data-invalid={Boolean(accountForm.formState.errors.display_name)}>
                      <FieldLabel htmlFor="display_name">{t("Display name")}</FieldLabel>
                      <Input
                        id="display_name"
                        aria-invalid={Boolean(accountForm.formState.errors.display_name)}
                        {...accountForm.register("display_name")}
                      />
                      {accountForm.formState.errors.display_name ? (
                        <FieldError>
                          {accountForm.formState.errors.display_name.message}
                        </FieldError>
                      ) : null}
                    </Field>
                    <Field data-invalid={Boolean(accountForm.formState.errors.email)}>
                      <FieldLabel htmlFor="email">{t("Email")}</FieldLabel>
                      <Input
                        id="email"
                        type="email"
                        aria-invalid={Boolean(accountForm.formState.errors.email)}
                        {...accountForm.register("email")}
                      />
                      {accountForm.formState.errors.email ? (
                        <FieldError>{accountForm.formState.errors.email.message}</FieldError>
                      ) : null}
                    </Field>
                    <Controller
                      control={accountForm.control}
                      name="role"
                      defaultValue={accountValues.role}
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
                      control={accountForm.control}
                      name="user_group_id"
                      defaultValue={accountValues.user_group_id}
                      render={({ field, fieldState }) => (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor="user_group_id">
                            {t("User group")}
                          </FieldLabel>
                          <Select value={field.value} onValueChange={field.onChange}>
                            <SelectTrigger
                              id="user_group_id"
                              aria-invalid={fieldState.invalid}
                            >
                              <SelectValue />
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
                      control={accountForm.control}
                      name="default_api_key_policy_id"
                      defaultValue={accountValues.default_api_key_policy_id}
                      render={({ field, fieldState }) => (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor="default_api_key_policy_id">
                            {t("API policy override")}
                          </FieldLabel>
                          <Select
                            value={field.value || "__inherit__"}
                            onValueChange={(value) =>
                              field.onChange(value === "__inherit__" ? "" : value)
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
                                <SelectItem value="__inherit__">
                                  {t("Inherit group policy")}
                                </SelectItem>
                                {policies.data
                                  ?.filter(
                                    (policy) =>
                                      policy.enabled ||
                                      policy.id ===
                                        accountValues.default_api_key_policy_id,
                                  )
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
                  <Button
                    type="submit"
                    className="self-start"
                    disabled={submitting !== null}
                  >
                    {submitting === "account" ? <Spinner data-icon="inline-start" /> : null}
                    {t("Save account details")}
                  </Button>
                </form>
              </CardContent>
            </Card>

            <UserPasswordRecoveryCard
              user={user}
              currentUserId={currentUser?.id}
              isPending={issueTemporaryPassword.isPending}
              onOpenTemporaryPassword={() => setTemporaryPasswordOpen(true)}
            />

            <Card>
              <CardHeader>
                <CardTitle>{t("Personal settings")}</CardTitle>
              </CardHeader>
              <CardContent>
                <Field orientation="horizontal">
                  <FieldContent>
                    <FieldLabel htmlFor="admin_user_websocket_enabled">
                      {t("Enable Responses WebSocket")}
                    </FieldLabel>
                    <FieldDescription>
                      {t(
                        "This preference applies to all active API keys owned by the user's account.",
                      )}
                    </FieldDescription>
                  </FieldContent>
                  <Switch
                    id="admin_user_websocket_enabled"
                    checked={websocketEnabled}
                    disabled={submitting !== null}
                    onCheckedChange={(checked) => {
                      const next = Boolean(checked);
                      setWebsocketEnabled(next);
                      setWebsocketDirty(Boolean(user && next !== user.websocket_enabled));
                    }}
                  />
                </Field>
                <Button
                  type="button"
                  className="mt-6"
                  disabled={
                    submitting !== null ||
                    websocketEnabled === user.websocket_enabled
                  }
                  onClick={() => void savePersonalSettings()}
                >
                  {submitting === "settings" ? (
                    <Spinner data-icon="inline-start" />
                  ) : null}
                  {t("Save personal settings")}
                </Button>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Balance management")}</CardTitle>
              </CardHeader>
              <CardContent>
                <form
                  onSubmit={balanceForm.handleSubmit(submitBalance, onInvalidBalance)}
                  className="flex max-w-xl flex-col gap-4"
                >
                  <DecimalField
                    id="balance_amount"
                    label={t("Balance")}
                    value={balanceForm.watch("balance_amount")}
                    onChange={(value) =>
                      balanceForm.setValue("balance_amount", value, {
                        shouldDirty: true,
                        shouldValidate: true,
                      })
                    }
                    error={balanceForm.formState.errors.balance_amount?.message}
                    required
                    description={t("Set the current account balance in USD.")}
                  />
                  <Button
                    type="submit"
                    className="self-start"
                    disabled={submitting !== null}
                  >
                    {submitting === "balance" ? <Spinner data-icon="inline-start" /> : null}
                    {t("Update balance")}
                  </Button>
                </form>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("Access status")}</CardTitle>
              </CardHeader>
              <CardContent>
                {user.can_reissue_invitation ? (
                  <Alert>
                    <AlertTitle>
                      {t(
                        user.status === "invited"
                          ? "Pending invitation activation"
                          : "Invitation recovery available",
                      )}
                    </AlertTitle>
                    <AlertDescription>
                      <div className="flex flex-col items-start gap-3">
                        <span>
                          {t(
                            user.status === "invited"
                              ? "This user must activate the invitation and set a password. Editing account details or balance keeps the invitation pending."
                              : "This account was disabled before setting a password. Reissue an invitation to restore the pending activation flow without changing its balance or settings.",
                          )}
                        </span>
                        <Button
                          type="button"
                          size="sm"
                          onClick={() => setReissueOpen(true)}
                          disabled={reissue.isPending}
                        >
                          {reissue.isPending ? <Spinner data-icon="inline-start" /> : null}
                          {t("Reissue invitation")}
                        </Button>
                      </div>
                    </AlertDescription>
                  </Alert>
                ) : isManageableStatus(user.status) ? (
                  <form
                    onSubmit={statusForm.handleSubmit(submitStatus)}
                    className="flex max-w-xl flex-col gap-4"
                  >
                    <Controller
                      control={statusForm.control}
                      name="status"
                      defaultValue={statusValues.status}
                      render={({ field, fieldState }) => (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor="status">{t("Status")}</FieldLabel>
                          <Select value={field.value} onValueChange={field.onChange}>
                            <SelectTrigger id="status" aria-invalid={fieldState.invalid}>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectGroup>
                                {manageableStatuses.map((status) => (
                                  <SelectItem key={status} value={status}>
                                    {userStatusLabel(status)}
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
                    <Button
                      type="submit"
                      className="self-start"
                      disabled={submitting !== null}
                    >
                      {submitting === "status" ? <Spinner data-icon="inline-start" /> : null}
                      {t("Update status")}
                    </Button>
                  </form>
                ) : (
                  <Alert variant="destructive">
                    <AlertTitle>{t("Unsupported account status")}</AlertTitle>
                    <AlertDescription>
                      {t("Reload the user before making access changes.")}
                    </AlertDescription>
                  </Alert>
                )}
              </CardContent>
            </Card>

          </>
        ) : null
      }
      dangerZone={
        user ? (
          <div className="flex flex-col items-start gap-4">
            {currentUser?.id === user.id ? (
              <Alert>
                <AlertTitle>{t("Current administrator account")}</AlertTitle>
                <AlertDescription>
                  {t("You cannot delete your own administrator account.")}
                </AlertDescription>
              </Alert>
            ) : null}
            <Button
              type="button"
              variant="destructive"
              disabled={
                remove.isPending ||
                submitting !== null ||
                currentUser?.id === user.id
              }
              onClick={() => setDeleteOpen(true)}
            >
              {remove.isPending ? <Spinner data-icon="inline-start" /> : null}
              {t("Delete user")}
            </Button>
          </div>
        ) : undefined
      }
      />
      <UserDetailDialogs
        passwordChangeRequired={user?.password_change_required}
        issueTemporaryPasswordPending={issueTemporaryPassword.isPending}
        deleteOpen={deleteOpen}
        onDeleteOpenChange={setDeleteOpen}
        onConfirmDelete={() => void deleteUser()}
        reissueOpen={reissueOpen}
        onReissueOpenChange={setReissueOpen}
        onConfirmReissue={() => void reissueInvitation()}
        temporaryPasswordOpen={temporaryPasswordOpen}
        onTemporaryPasswordOpenChange={(open) => {
          setTemporaryPasswordOpen(open);
          if (!open) setAdminPassword("");
        }}
        adminPassword={adminPassword}
        onAdminPasswordChange={setAdminPassword}
        onConfirmTemporaryPassword={() => void generateTemporaryPassword()}
        invitationToken={invitationToken}
        temporaryPassword={temporaryPassword}
        onCloseInvitationToken={() => setInvitationToken(null)}
        onCloseTemporaryPassword={() => setTemporaryPassword(null)}
      />
    </>
  );
}
