import { useEffect, useState } from "react";
import { Link, Navigate, useNavigate } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { login } from "@/api/session";
import { useSession } from "@/lib/use-session";
import { useI18n } from "@/app/i18n";

export function LoginPage() {
  const { isAuthenticated } = useSession();
  const navigate = useNavigate();
  const { t } = useI18n();
  const schema = z.object({
    email: z.string().email(t("Enter a valid email.")),
    password: z.string().min(12, t("Password must be at least 12 characters.")),
  });
  type FormValues = z.infer<typeof schema>;
  const [submitting, setSubmitting] = useState(false);
  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<FormValues>({ resolver: zodResolver(schema) });

  useEffect(() => {
    if (isAuthenticated) navigate("/account", { replace: true });
  }, [isAuthenticated, navigate]);

  if (isAuthenticated) return <Navigate to="/account" replace />;

  const onSubmit = async (values: FormValues) => {
    setSubmitting(true);
    try {
      await login(values);
      toast.success(t("Signed in"));
      navigate("/account", { replace: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Sign in failed"));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Sign in")}</CardTitle>
        <CardDescription>{t("Use your Console account to continue.")}</CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
          <FieldGroup>
            <Field data-invalid={Boolean(errors.email)}>
              <FieldLabel htmlFor="email">{t("Email")}</FieldLabel>
              <Input
                id="email"
                type="email"
                autoComplete="email"
                aria-invalid={Boolean(errors.email)}
                {...register("email")}
              />
              {errors.email ? <FieldError>{errors.email.message}</FieldError> : null}
            </Field>
            <Field data-invalid={Boolean(errors.password)}>
              <FieldLabel htmlFor="password">{t("Password")}</FieldLabel>
              <Input
                id="password"
                type="password"
                autoComplete="current-password"
                aria-invalid={Boolean(errors.password)}
                {...register("password")}
              />
              {errors.password ? <FieldError>{errors.password.message}</FieldError> : null}
            </Field>
          </FieldGroup>
          <Button type="submit" disabled={submitting}>
            {submitting ? <Spinner data-icon="inline-start" /> : null}
            {t("Sign in")}
          </Button>
        </form>
        <div className="mt-4 flex flex-col gap-1 text-center text-xs text-muted-foreground">
          <p>
            {t("Have a registration code?")}{" "}
            <Button
              variant="link"
              size="xs"
              render={<Link to="/register" />}
              nativeButton={false}
            >
              {t("Create account")}
            </Button>
          </p>
          <p>
            {t("Received a personal invitation?")}{" "}
            <Button
              variant="link"
              size="xs"
              render={<Link to="/activate-invitation" />}
              nativeButton={false}
            >
              {t("Activate it")}
            </Button>
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
