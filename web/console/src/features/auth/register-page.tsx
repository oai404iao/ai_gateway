import { useState } from "react";
import { Link, Navigate, useNavigate } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import { registerAccount } from "@/api/session";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { useSession } from "@/lib/use-session";

export function RegisterPage() {
  const { isAuthenticated } = useSession();
  const navigate = useNavigate();
  const { t } = useI18n();
  const schema = z
    .object({
      invitation_code: z
        .string()
        .trim()
        .min(12, t("Invitation code must be at least 12 characters."))
        .max(128, t("Invitation code must be at most 128 characters."))
        .refine((value) => !/\s/.test(value), t("Invitation code cannot contain whitespace.")),
      email: z.string().email(t("Enter a valid email.")),
      display_name: z.string().trim().min(1, t("Display name is required.")).max(200),
      password: z.string().min(12, t("Password must be at least 12 characters.")),
      confirm_password: z.string(),
    })
    .refine((value) => value.password === value.confirm_password, {
      path: ["confirm_password"],
      message: t("Passwords do not match."),
    });
  type FormValues = z.infer<typeof schema>;
  const [submitting, setSubmitting] = useState(false);
  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      invitation_code: "",
      email: "",
      display_name: "",
      password: "",
      confirm_password: "",
    },
  });

  if (isAuthenticated) return <Navigate to="/account" replace />;

  const onSubmit = async (values: FormValues) => {
    setSubmitting(true);
    try {
      await registerAccount({
        invitation_code: values.invitation_code,
        email: values.email,
        display_name: values.display_name,
        password: values.password,
      });
      toast.success(t("Account created"));
      navigate("/account", { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.code === "invalid_registration_code") {
        toast.error(t("The invitation code is invalid, expired, disabled, or exhausted."));
      } else if (
        error instanceof ApiError &&
        error.code === "registration_email_conflict"
      ) {
        toast.error(t("An account with this email already exists."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Registration failed"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Create account")}</CardTitle>
        <CardDescription>
          {t("Use a registration invitation code to create your Console account.")}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
          <FieldGroup>
            <Field data-invalid={Boolean(errors.invitation_code)}>
              <FieldLabel htmlFor="registration_invitation_code">
                {t("Invitation code")}
              </FieldLabel>
              <Input
                id="registration_invitation_code"
                autoComplete="off"
                aria-invalid={Boolean(errors.invitation_code)}
                {...register("invitation_code")}
              />
              {errors.invitation_code ? (
                <FieldError>{errors.invitation_code.message}</FieldError>
              ) : null}
            </Field>
            <Field data-invalid={Boolean(errors.email)}>
              <FieldLabel htmlFor="registration_email">{t("Email")}</FieldLabel>
              <Input
                id="registration_email"
                type="email"
                autoComplete="email"
                aria-invalid={Boolean(errors.email)}
                {...register("email")}
              />
              {errors.email ? <FieldError>{errors.email.message}</FieldError> : null}
            </Field>
            <Field data-invalid={Boolean(errors.display_name)}>
              <FieldLabel htmlFor="registration_display_name">
                {t("Display name")}
              </FieldLabel>
              <Input
                id="registration_display_name"
                autoComplete="name"
                aria-invalid={Boolean(errors.display_name)}
                {...register("display_name")}
              />
              {errors.display_name ? (
                <FieldError>{errors.display_name.message}</FieldError>
              ) : null}
            </Field>
            <Field data-invalid={Boolean(errors.password)}>
              <FieldLabel htmlFor="registration_password">{t("Password")}</FieldLabel>
              <Input
                id="registration_password"
                type="password"
                autoComplete="new-password"
                aria-invalid={Boolean(errors.password)}
                {...register("password")}
              />
              {errors.password ? <FieldError>{errors.password.message}</FieldError> : null}
            </Field>
            <Field data-invalid={Boolean(errors.confirm_password)}>
              <FieldLabel htmlFor="registration_confirm_password">
                {t("Confirm password")}
              </FieldLabel>
              <Input
                id="registration_confirm_password"
                type="password"
                autoComplete="new-password"
                aria-invalid={Boolean(errors.confirm_password)}
                {...register("confirm_password")}
              />
              {errors.confirm_password ? (
                <FieldError>{errors.confirm_password.message}</FieldError>
              ) : null}
            </Field>
          </FieldGroup>
          <Button type="submit" disabled={submitting}>
            {submitting ? <Spinner data-icon="inline-start" /> : null}
            {t("Create account")}
          </Button>
        </form>
        <p className="mt-4 text-center text-xs text-muted-foreground">
          {t("Already have an account?")}{" "}
          <Button
            variant="link"
            size="xs"
            render={<Link to="/login" />}
            nativeButton={false}
          >
            {t("Sign in")}
          </Button>
        </p>
      </CardContent>
    </Card>
  );
}
