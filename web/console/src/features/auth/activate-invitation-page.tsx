import { useState } from "react";
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
import { activateInvitation } from "@/api/session";
import { useSession } from "@/lib/use-session";

const schema = z.object({
  invitation_token: z.string().min(1, "Invitation token is required."),
  password: z.string().min(12, "Password must be at least 12 characters."),
});

type FormValues = z.infer<typeof schema>;

export function ActivateInvitationPage() {
  const { isAuthenticated } = useSession();
  const navigate = useNavigate();
  const [submitting, setSubmitting] = useState(false);
  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<FormValues>({ resolver: zodResolver(schema) });

  if (isAuthenticated) return <Navigate to="/account" replace />;

  const onSubmit = async (values: FormValues) => {
    setSubmitting(true);
    try {
      await activateInvitation(values);
      toast.success("Account activated");
      navigate("/account", { replace: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Activation failed");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>Activate invitation</CardTitle>
        <CardDescription>
          Set your password to activate the Console account you were invited to.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="invitation_token">Invitation token</FieldLabel>
              <Input id="invitation_token" autoComplete="off" {...register("invitation_token")} />
              {errors.invitation_token ? (
                <FieldError>{errors.invitation_token.message}</FieldError>
              ) : null}
            </Field>
            <Field>
              <FieldLabel htmlFor="password">New password</FieldLabel>
              <Input
                id="password"
                type="password"
                autoComplete="new-password"
                {...register("password")}
              />
              {errors.password ? <FieldError>{errors.password.message}</FieldError> : null}
            </Field>
          </FieldGroup>
          <Button type="submit" disabled={submitting}>
            {submitting ? <Spinner data-icon="inline-start" /> : null}
            Activate account
          </Button>
        </form>
        <p className="mt-4 text-center text-xs text-muted-foreground">
          Already have an account?{" "}
          <Link className="font-medium text-foreground underline" to="/login">
            Sign in
          </Link>
        </p>
      </CardContent>
    </Card>
  );
}
