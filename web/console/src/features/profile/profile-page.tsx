import { useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Separator } from "@/components/ui/separator";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChangePassword, useProfile, useUpdateProfile } from "@/features/profile/api";
import { formatCurrency } from "@/lib/formatters";
import { formatDateTime as formatTs } from "@/lib/dates";
import { roleLabel } from "@/lib/permissions";

const profileSchema = z.object({
  display_name: z.string().min(1, "Display name is required.").max(200),
});

type ProfileValues = z.infer<typeof profileSchema>;

const passwordSchema = z
  .object({
    current_password: z.string().min(12, "At least 12 characters."),
    new_password: z.string().min(12, "At least 12 characters."),
    confirm_password: z.string().min(12, "At least 12 characters."),
  })
  .refine((values) => values.new_password === values.confirm_password, {
    path: ["confirm_password"],
    message: "Passwords do not match.",
  });

type PasswordValues = z.infer<typeof passwordSchema>;

export function ProfilePage() {
  const { data: profile, isLoading, error } = useProfile();
  const updateProfile = useUpdateProfile();
  const changePassword = useChangePassword();

  const profileForm = useForm<ProfileValues>({
    resolver: zodResolver(profileSchema),
    values: profile ? { display_name: profile.display_name } : undefined,
  });
  const passwordForm = useForm<PasswordValues>({ resolver: zodResolver(passwordSchema) });
  const [savingProfile, setSavingProfile] = useState(false);
  const [savingPassword, setSavingPassword] = useState(false);

  const onProfile = async (values: ProfileValues) => {
    setSavingProfile(true);
    try {
      await updateProfile.mutateAsync(values);
      toast.success("Profile updated");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Update failed");
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
      toast.success("Password changed. All sessions were signed out.");
      passwordForm.reset();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Password change failed");
    } finally {
      setSavingPassword(false);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Profile" description="Your Console identity and security settings." />
      <AsyncResource isLoading={isLoading} error={error}>
        {profile ? (
          <Card>
            <CardHeader>
              <CardTitle>Account</CardTitle>
              <CardDescription>Read-only account facts.</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField label="Email" value={profile.email ?? "—"} />
                <DetailField label="Role" value={roleLabel(profile.role)} />
                <DetailField label="Status" value={<StatusBadge value={profile.status} />} />
                <DetailField
                  label="Balance"
                  value={formatCurrency(profile.balance_amount, profile.currency)}
                />
                <DetailField label="Created" value={formatTs(profile.created_at)} />
                <DetailField label="Updated" value={formatTs(profile.updated_at)} />
              </dl>
            </CardContent>
          </Card>
        ) : null}
      </AsyncResource>

      <Card>
        <CardHeader>
          <CardTitle>Display name</CardTitle>
          <CardDescription>Shown to administrators and in audit records.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={profileForm.handleSubmit(onProfile)} className="flex flex-col gap-4">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="display_name">Display name</FieldLabel>
                <Input id="display_name" {...profileForm.register("display_name")} />
                {profileForm.formState.errors.display_name ? (
                  <FieldError>{profileForm.formState.errors.display_name.message}</FieldError>
                ) : null}
              </Field>
            </FieldGroup>
            <Button type="submit" className="self-start" disabled={savingProfile}>
              {savingProfile ? <Spinner data-icon="inline-start" /> : null}
              Save display name
            </Button>
          </form>
        </CardContent>
      </Card>

      <Separator />

      <Card>
        <CardHeader>
          <CardTitle>Change password</CardTitle>
          <CardDescription>
            Changing your password immediately signs out every active session.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={passwordForm.handleSubmit(onPassword)} className="flex flex-col gap-4">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="current_password">Current password</FieldLabel>
                <Input
                  id="current_password"
                  type="password"
                  autoComplete="current-password"
                  {...passwordForm.register("current_password")}
                />
                {passwordForm.formState.errors.current_password ? (
                  <FieldError>{passwordForm.formState.errors.current_password.message}</FieldError>
                ) : null}
              </Field>
              <Field>
                <FieldLabel htmlFor="new_password">New password</FieldLabel>
                <Input
                  id="new_password"
                  type="password"
                  autoComplete="new-password"
                  {...passwordForm.register("new_password")}
                />
                {passwordForm.formState.errors.new_password ? (
                  <FieldError>{passwordForm.formState.errors.new_password.message}</FieldError>
                ) : null}
              </Field>
              <Field>
                <FieldLabel htmlFor="confirm_password">Confirm new password</FieldLabel>
                <Input
                  id="confirm_password"
                  type="password"
                  autoComplete="new-password"
                  {...passwordForm.register("confirm_password")}
                />
                {passwordForm.formState.errors.confirm_password ? (
                  <FieldError>{passwordForm.formState.errors.confirm_password.message}</FieldError>
                ) : null}
              </Field>
            </FieldGroup>
            <Button type="submit" className="self-start" disabled={savingPassword}>
              {savingPassword ? <Spinner data-icon="inline-start" /> : null}
              Change password
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
