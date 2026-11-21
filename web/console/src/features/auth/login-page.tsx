import { useState } from "react";
import { Navigate } from "react-router";
import { ArrowRight, Eye, EyeOff } from "lucide-react";
import { NavigationLink } from "@/components/shared/navigation-link";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { AuthFormPage } from "./auth-form-page";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { login } from "@/api/session";
import { useSession } from "@/lib/use-session";
import { useI18n } from "@/app/i18n";

export function LoginPage() {
  const { isAuthenticated, user: sessionUser } = useSession();
  const [showPassword, setShowPassword] = useState(false);
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

  if (isAuthenticated) {
    return (
      <Navigate to={sessionUser?.password_change_required ? "/change-password" : "/"} replace />
    );
  }

  const onSubmit = async (values: FormValues) => {
    setSubmitting(true);
    try {
      await login(values);
      toast.success(t("Signed in"));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Sign in failed"));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <AuthFormPage title={t("Sign in to the gateway")}>
      <form onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
        <FieldGroup>
          <Field data-invalid={Boolean(errors.email)}>
            <FieldLabel htmlFor="email">{t("Email")}</FieldLabel>
            <Input
              id="email"
              className="h-10"
              type="email"
              autoComplete="email"
              aria-invalid={Boolean(errors.email)}
              {...register("email")}
            />
            {errors.email ? <FieldError>{errors.email.message}</FieldError> : null}
          </Field>
          <Field data-invalid={Boolean(errors.password)}>
            <FieldLabel htmlFor="password">{t("Password")}</FieldLabel>
            <InputGroup className="h-10">
              <InputGroupInput
                id="password"
                type={showPassword ? "text" : "password"}
                autoComplete="current-password"
                aria-invalid={Boolean(errors.password)}
                {...register("password")}
              />
              <InputGroupAddon align="inline-end">
                <InputGroupButton
                  size="icon-sm"
                  aria-label={t(showPassword ? "Hide password" : "Show password")}
                  onClick={() => setShowPassword(!showPassword)}
                >
                  {showPassword ? <EyeOff /> : <Eye />}
                </InputGroupButton>
              </InputGroupAddon>
            </InputGroup>
            {errors.password ? <FieldError>{errors.password.message}</FieldError> : null}
          </Field>
        </FieldGroup>
        <Button type="submit" size="lg" disabled={submitting}>
          {submitting ? <Spinner data-icon="inline-start" /> : null}
          {t("Sign in")}
          {!submitting && <ArrowRight data-icon="inline-end" />}
        </Button>
      </form>
      <div className="mt-6 flex flex-col gap-2 border-t pt-4 text-xs text-muted-foreground">
        <div className="flex flex-wrap items-center gap-x-3">
          <NavigationLink variant="link" size="xs" to="/register">
            {t("Register with a code")}
          </NavigationLink>
          <NavigationLink variant="link" size="xs" to="/activate-invitation">
            {t("Activate invitation")}
          </NavigationLink>
        </div>
        <details>
          <summary className="w-fit cursor-pointer rounded-sm py-1">
            {t("Forgot your password?")}
          </summary>
          <p className="mt-1">{t("Ask an administrator to generate a temporary password.")}</p>
        </details>
      </div>
    </AuthFormPage>
  );
}
