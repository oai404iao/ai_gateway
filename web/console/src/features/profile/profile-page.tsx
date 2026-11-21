import { useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { AccountPage } from "./account-page";
import { accountInitials } from "@/lib/account-initials";
import { AsyncResource } from "@/components/shared/async-resource";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChangePassword, useProfile, useUpdateProfile } from "@/features/profile/api";
import { formatUsd } from "@/lib/formatters";
import { formatDateTime as formatTs } from "@/lib/dates";
import { roleLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

export function ProfilePage() {
  const { data: profile, isLoading, error } = useProfile();
  const updateProfile = useUpdateProfile();
  const changePassword = useChangePassword();
  const { t } = useI18n();
  const profileSchema = z.object({
    display_name: z.string().min(1, t("Display name is required.")).max(200),
  });
  type ProfileValues = z.infer<typeof profileSchema>;
  const passwordSchema = z
    .object({
      current_password: z.string().min(12, t("At least 12 characters.")),
      new_password: z.string().min(12, t("At least 12 characters.")),
      confirm_password: z.string().min(12, t("At least 12 characters.")),
    })
    .refine((values) => values.new_password === values.confirm_password, {
      path: ["confirm_password"],
      message: t("Passwords do not match."),
    });
  type PasswordValues = z.infer<typeof passwordSchema>;

  const profileForm = useForm<ProfileValues>({
    resolver: zodResolver(profileSchema),
    defaultValues: { display_name: "" },
    values: profile ? { display_name: profile.display_name } : undefined,
  });
  const passwordForm = useForm<PasswordValues>({
    resolver: zodResolver(passwordSchema),
    defaultValues: { current_password: "", new_password: "", confirm_password: "" },
  });
  const [savingProfile, setSavingProfile] = useState(false);
  const [savingPassword, setSavingPassword] = useState(false);
  const [passwordOpen, setPasswordOpen] = useState(false);
  const draft = useConfigurationDraft(
    savingProfile || savingPassword,
    profileForm.formState.isDirty || passwordForm.formState.isDirty,
  );

  const onProfile = async (values: ProfileValues) => {
    setSavingProfile(true);
    try {
      await updateProfile.mutateAsync(values);
      profileForm.reset(values);
      toast.success(t("Profile updated"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Update failed"));
    } finally {
      setSavingProfile(false);
    }
  };

  const onPassword = async (values: PasswordValues) => {
    setSavingPassword(true);
    try {
      await changePassword.mutateAsync({
        current_password: values.current_password,
        new_password: values.new_password,
      });
      toast.success(t("Password changed. All sessions were signed out."));
      passwordForm.reset();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Password change failed"));
    } finally {
      setSavingPassword(false);
    }
  };

  return (
    <AccountPage>
      {draft.navigationGuard}
      <AsyncResource isLoading={isLoading} error={error}>
        {profile ? (
          <section>
            <header className="mb-4 flex items-center gap-3">
              <Avatar size="lg" aria-hidden="true">
                <AvatarFallback>
                  {accountInitials(profile.display_name)}
                </AvatarFallback>
              </Avatar>
              <div className="min-w-0 break-words">
                <h2 className="text-lg font-semibold">{profile.display_name}</h2>
                <p className="text-sm text-muted-foreground">{profile.email ?? "—"}</p>
              </div>
            </header>
            <form
              onSubmit={profileForm.handleSubmit(onProfile)}
              className="flex flex-col gap-2 border-b py-4 sm:flex-row sm:items-start sm:justify-between"
            >
              <div className="flex flex-col gap-1">
                <FieldLabel htmlFor="display_name">{t("Display name")}</FieldLabel>
                <p className="text-xs text-muted-foreground">
                  {t("Shown to administrators and in audit records.")}
                </p>
              </div>
              <div className="flex min-w-0 items-start gap-2">
                <Field data-invalid={Boolean(profileForm.formState.errors.display_name)}>
                  <Input
                    id="display_name"
                    aria-invalid={Boolean(profileForm.formState.errors.display_name)}
                    {...profileForm.register("display_name")}
                  />
                  {profileForm.formState.errors.display_name ? (
                    <FieldError>{profileForm.formState.errors.display_name.message}</FieldError>
                  ) : null}
                </Field>
                <Button
                  type="submit"
                  variant="outline"
                  disabled={savingProfile || !profileForm.formState.isDirty}
                >
                  {savingProfile ? <Spinner data-icon="inline-start" /> : null}
                  {t("Save display name")}
                </Button>
              </div>
            </form>
            <dl className="divide-y text-sm *:flex *:items-center *:justify-between *:gap-3 *:py-3 [&_dt]:text-muted-foreground">
              <div>
                <dt>{t("Role")}</dt>
                <dd>{roleLabel(profile.role)}</dd>
              </div>
              <div>
                <dt>{t("Status")}</dt>
                <dd>
                  <StatusBadge value={profile.status} />
                </dd>
              </div>
              <div>
                <dt>{t("Balance")}</dt>
                <dd>{formatUsd(profile.balance_amount)}</dd>
              </div>
            </dl>
            <details className="border-t py-2 text-xs text-muted-foreground">
              <summary className="w-fit cursor-pointer py-1">{t("Profile timestamps")}</summary>
              <dl className="mt-2 grid gap-2">
                <div className="flex justify-between gap-3">
                  <dt>{t("Created")}</dt>
                  <dd>{formatTs(profile.created_at)}</dd>
                </div>
                <div className="flex justify-between gap-3">
                  <dt>{t("Updated")}</dt>
                  <dd>{formatTs(profile.updated_at)}</dd>
                </div>
              </dl>
            </details>
          </section>
        ) : null}
      </AsyncResource>

      <section>
        <h2 className="mb-2 text-lg font-semibold">{t("Security")}</h2>
        <Dialog
          open={passwordOpen}
          onOpenChange={(open) => {
            if (savingPassword) return;
            if (!open) passwordForm.reset();
            setPasswordOpen(open);
          }}
        >
          <div className="flex flex-wrap items-center justify-between gap-3 border-b py-3">
            <h3 className="text-sm font-medium">{t("Password")}</h3>
            <DialogTrigger render={<Button variant="outline" />}>
              {t("Change password")}
            </DialogTrigger>
          </div>
          <DialogContent className="max-h-[90dvh] overflow-y-auto">
            <DialogHeader>
              <DialogTitle>{t("Change password")}</DialogTitle>
              <DialogDescription>
                {t("Changing your password immediately signs out every active session.")}
              </DialogDescription>
            </DialogHeader>
            <form onSubmit={passwordForm.handleSubmit(onPassword)} className="flex flex-col gap-4">
              <FieldGroup>
                <Field data-invalid={Boolean(passwordForm.formState.errors.current_password)}>
                  <FieldLabel htmlFor="current_password">{t("Current password")}</FieldLabel>
                  <Input
                    id="current_password"
                    type="password"
                    autoComplete="current-password"
                    aria-invalid={Boolean(passwordForm.formState.errors.current_password)}
                    {...passwordForm.register("current_password")}
                  />
                  {passwordForm.formState.errors.current_password ? (
                    <FieldError>
                      {passwordForm.formState.errors.current_password.message}
                    </FieldError>
                  ) : null}
                </Field>
                <Field data-invalid={Boolean(passwordForm.formState.errors.new_password)}>
                  <FieldLabel htmlFor="new_password">{t("New password")}</FieldLabel>
                  <Input
                    id="new_password"
                    type="password"
                    autoComplete="new-password"
                    aria-invalid={Boolean(passwordForm.formState.errors.new_password)}
                    {...passwordForm.register("new_password")}
                  />
                  {passwordForm.formState.errors.new_password ? (
                    <FieldError>{passwordForm.formState.errors.new_password.message}</FieldError>
                  ) : null}
                </Field>
                <Field data-invalid={Boolean(passwordForm.formState.errors.confirm_password)}>
                  <FieldLabel htmlFor="confirm_password">{t("Confirm new password")}</FieldLabel>
                  <Input
                    id="confirm_password"
                    type="password"
                    autoComplete="new-password"
                    aria-invalid={Boolean(passwordForm.formState.errors.confirm_password)}
                    {...passwordForm.register("confirm_password")}
                  />
                  {passwordForm.formState.errors.confirm_password ? (
                    <FieldError>
                      {passwordForm.formState.errors.confirm_password.message}
                    </FieldError>
                  ) : null}
                </Field>
              </FieldGroup>
              <DialogFooter>
                <DialogClose
                  render={<Button type="button" variant="outline" disabled={savingPassword} />}
                >
                  {t("Cancel")}
                </DialogClose>
                <Button type="submit" disabled={savingPassword}>
                  {savingPassword ? <Spinner data-icon="inline-start" /> : null}
                  {t("Change password")}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        </Dialog>
      </section>
    </AccountPage>
  );
}
