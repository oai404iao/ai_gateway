import type { ControlPlaneUser, TemporaryPasswordResponse } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { SecretOnceDialog } from "@/components/shared/secret-once-dialog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { formatDateTime } from "@/lib/dates";

interface UserPasswordRecoveryCardProps {
  user: ControlPlaneUser;
  currentUserId?: string;
  isPending: boolean;
  onOpenTemporaryPassword: () => void;
}

export function UserPasswordRecoveryCard({
  user,
  currentUserId,
  isPending,
  onOpenTemporaryPassword,
}: UserPasswordRecoveryCardProps) {
  const { t } = useI18n();
  if (user.can_reissue_invitation) return null;

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Password recovery")}</CardTitle>
        <CardDescription>
          {t(
            "Generate an expiring temporary password that forces the user to choose a new password.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col items-start gap-4">
        {user.password_change_required ? (
          <Alert>
            <AlertTitle>{t("Password change pending")}</AlertTitle>
            <AlertDescription>
              {user.temporary_password_expires_at
                ? t("The current temporary password expires at {time}.", {
                    time: formatDateTime(user.temporary_password_expires_at),
                  })
                : t("The user must replace the temporary password.")}
            </AlertDescription>
          </Alert>
        ) : null}
        {currentUserId === user.id ? (
          <Alert>
            <AlertTitle>{t("Current administrator account")}</AlertTitle>
            <AlertDescription>
              {t(
                "Use another administrator or the host recovery command to reset your own password.",
              )}
            </AlertDescription>
          </Alert>
        ) : user.status !== "active" ? (
          <Alert>
            <AlertTitle>{t("Active account required")}</AlertTitle>
            <AlertDescription>
              {t(
                "Restore this account to active status before generating a temporary password.",
              )}
            </AlertDescription>
          </Alert>
        ) : (
          <>
            <Alert>
              <AlertTitle>{t("Existing access will be revoked")}</AlertTitle>
              <AlertDescription>
                {t(
                  "Generating a temporary password immediately invalidates the current password and every Console login session. API keys are unchanged.",
                )}
              </AlertDescription>
            </Alert>
            <Button
              type="button"
              onClick={onOpenTemporaryPassword}
              disabled={isPending}
            >
              {isPending ? <Spinner data-icon="inline-start" /> : null}
              {t(
                user.password_change_required
                  ? "Generate replacement temporary password"
                  : "Generate temporary password",
              )}
            </Button>
          </>
        )}
      </CardContent>
    </Card>
  );
}

interface UserDetailDialogsProps {
  passwordChangeRequired?: boolean;
  issueTemporaryPasswordPending: boolean;
  deleteOpen: boolean;
  onDeleteOpenChange: (open: boolean) => void;
  onConfirmDelete: () => void;
  reissueOpen: boolean;
  onReissueOpenChange: (open: boolean) => void;
  onConfirmReissue: () => void;
  temporaryPasswordOpen: boolean;
  onTemporaryPasswordOpenChange: (open: boolean) => void;
  adminPassword: string;
  onAdminPasswordChange: (value: string) => void;
  onConfirmTemporaryPassword: () => void;
  invitationToken: string | null;
  temporaryPassword: TemporaryPasswordResponse | null;
  onCloseInvitationToken: () => void;
  onCloseTemporaryPassword: () => void;
}

export function UserDetailDialogs({
  passwordChangeRequired,
  issueTemporaryPasswordPending,
  deleteOpen,
  onDeleteOpenChange,
  onConfirmDelete,
  reissueOpen,
  onReissueOpenChange,
  onConfirmReissue,
  temporaryPasswordOpen,
  onTemporaryPasswordOpenChange,
  adminPassword,
  onAdminPasswordChange,
  onConfirmTemporaryPassword,
  invitationToken,
  temporaryPassword,
  onCloseInvitationToken,
  onCloseTemporaryPassword,
}: UserDetailDialogsProps) {
  const { t } = useI18n();
  return (
    <>
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={onDeleteOpenChange}
        title={t("Delete user?")}
        description={t(
          "This anonymizes the account, revokes every session and invitation, and deletes every API key with its stored secret. Request logs and audit history are preserved. This action cannot be undone.",
        )}
        confirmLabel={t("Delete user")}
        destructive
        onConfirm={onConfirmDelete}
      />
      <ConfirmDialog
        open={reissueOpen}
        onOpenChange={onReissueOpenChange}
        title={t("Reissue invitation?")}
        description={t(
          "All previous invitation tokens for this user will stop working. A new token will be shown once.",
        )}
        confirmLabel={t("Reissue invitation")}
        onConfirm={onConfirmReissue}
      />
      <ConfirmDialog
        open={temporaryPasswordOpen}
        onOpenChange={onTemporaryPasswordOpenChange}
        title={t(
          passwordChangeRequired
            ? "Generate replacement temporary password?"
            : "Generate temporary password?",
        )}
        description={t(
          "Re-enter your administrator password. The target user's current password and Console sessions will stop working immediately.",
        )}
        content={
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="temporary-password-admin-confirmation">
                {t("Your current password")}
              </FieldLabel>
              <Input
                id="temporary-password-admin-confirmation"
                type="password"
                autoComplete="current-password"
                value={adminPassword}
                onChange={(event) => onAdminPasswordChange(event.target.value)}
              />
            </Field>
          </FieldGroup>
        }
        confirmLabel={t(
          passwordChangeRequired
            ? "Generate replacement password"
            : "Generate temporary password",
        )}
        confirmDisabled={adminPassword.length < 12 || issueTemporaryPasswordPending}
        onConfirm={onConfirmTemporaryPassword}
      />
      <SecretOnceDialog
        open={Boolean(invitationToken)}
        onOpenChange={(open) => !open && onCloseInvitationToken()}
        title={t("Replacement invitation token")}
        description={t("Give this new token to the user to activate their account.")}
        secret={invitationToken}
      />
      <SecretOnceDialog
        open={Boolean(temporaryPassword)}
        onOpenChange={(open) => !open && onCloseTemporaryPassword()}
        title={t("Temporary password")}
        description={
          temporaryPassword
            ? t(
                "Give this password to the user through a secure channel. It expires at {time} and will stop working immediately after the user sets a new password.",
                { time: formatDateTime(temporaryPassword.expires_at) },
              )
            : undefined
        }
        secret={temporaryPassword?.temporary_password ?? null}
      />
    </>
  );
}
