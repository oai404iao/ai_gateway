import { useState } from "react";
import { useNavigate } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { completePasswordReset, logout } from "@/api/session";
import { useSession } from "@/lib/use-session";
import { useI18n } from "@/app/i18n";

export function CompletePasswordResetPage() {
  const { user: sessionUser } = useSession();
  const navigate = useNavigate();
  const { t } = useI18n();
  const [submitting, setSubmitting] = useState(false);
  const [signingOut, setSigningOut] = useState(false);
  const schema = z
    .object({
      new_password: z.string().min(12, t("Password must be at least 12 characters.")),
      confirm_password: z.string().min(12, t("Password must be at least 12 characters.")),
    })
    .refine((values) => values.new_password === values.confirm_password, {
      path: ["confirm_password"],
      message: t("Passwords do not match."),
    });
  type FormValues = z.infer<typeof schema>;
  const form = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      new_password: "",
      confirm_password: "",
    },
  });

  const onSubmit = async (values: FormValues) => {
    setSubmitting(true);
    try {
      await completePasswordReset({ new_password: values.new_password });
      toast.success(t("Password updated"));
      navigate("/account", { replace: true });
    } catch (error) {
      toast.error(
        error instanceof ApiError && error.code === "new_password_matches_temporary"
          ? t("The new password must differ from the temporary password.")
          : error instanceof Error
            ? error.message
            : t("Password change failed"),
      );
    } finally {
      setSubmitting(false);
    }
  };

  const onSignOut = async () => {
    setSigningOut(true);
    try {
      await logout();
      navigate("/login", { replace: true });
    } finally {
      setSigningOut(false);
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Set a new password")}</CardTitle>
        <CardDescription>
          {t("Your temporary password only grants access to this password-change flow.")}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
          <Alert>
            <AlertTitle>{t("Password change required")}</AlertTitle>
            <AlertDescription>
              {sessionUser?.temporary_password_expires_at
                ? t("The temporary password expires at {time}.", {
                    time: new Date(
                      sessionUser.temporary_password_expires_at,
                    ).toLocaleString(),
                  })
                : t("Choose a permanent password before continuing.")}
            </AlertDescription>
          </Alert>
          <FieldGroup>
            <Field data-invalid={Boolean(form.formState.errors.new_password)}>
              <FieldLabel htmlFor="new_password">{t("New password")}</FieldLabel>
              <Input
                id="new_password"
                type="password"
                autoComplete="new-password"
                aria-invalid={Boolean(form.formState.errors.new_password)}
                {...form.register("new_password")}
              />
              {form.formState.errors.new_password ? (
                <FieldError>{form.formState.errors.new_password.message}</FieldError>
              ) : null}
            </Field>
            <Field data-invalid={Boolean(form.formState.errors.confirm_password)}>
              <FieldLabel htmlFor="confirm_password">
                {t("Confirm new password")}
              </FieldLabel>
              <Input
                id="confirm_password"
                type="password"
                autoComplete="new-password"
                aria-invalid={Boolean(form.formState.errors.confirm_password)}
                {...form.register("confirm_password")}
              />
              {form.formState.errors.confirm_password ? (
                <FieldError>{form.formState.errors.confirm_password.message}</FieldError>
              ) : null}
            </Field>
          </FieldGroup>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={submitting || signingOut}>
              {submitting ? <Spinner data-icon="inline-start" /> : null}
              {t("Save new password")}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={submitting || signingOut}
              onClick={() => void onSignOut()}
            >
              {signingOut ? <Spinner data-icon="inline-start" /> : null}
              {t("Sign out")}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
