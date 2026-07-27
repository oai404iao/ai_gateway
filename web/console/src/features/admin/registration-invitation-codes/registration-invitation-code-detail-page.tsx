import { useEffect, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useNavigate, useParams } from "react-router";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import type {
  RegistrationInvitationCodeCreateInput,
  RegistrationInvitationCodeUpdateInput,
  RegistrationInvitationCodeView,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Field,
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
import {
  DecimalField,
  NullableNumberField,
} from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { SecretOnceDialog } from "@/components/shared/secret-once-dialog";
import { StatusBadge } from "@/components/shared/status-badge";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import {
  useCreateRegistrationInvitationCode,
  useRegistrationInvitationCode,
  useUpdateRegistrationInvitationCode,
  useUserGroups,
} from "@/features/admin/api";
import {
  dateTimeLocalToIso,
  formatDateTime,
  formatDateTimeLocalInput,
  formatExpiry,
} from "@/lib/dates";
import { formatUsd } from "@/lib/formatters";

const baseSchema = z.object({
  name: z.string().trim().min(1, "Name is required.").max(100),
  invitation_code: z.string(),
  max_uses: z.number().int().positive("Maximum uses must be a positive integer.").nullable(),
  expires_at: z.string(),
  enabled: z.boolean(),
  user_group_id: z.string().min(1, "Pick a user group."),
  initial_balance_amount: z
    .string()
    .regex(/^\d+(?:\.\d+)?$/, "Enter a valid non-negative balance."),
});

type FormValues = z.infer<typeof baseSchema>;

const empty: FormValues = {
  name: "",
  invitation_code: "",
  max_uses: null,
  expires_at: "",
  enabled: true,
  user_group_id: "",
  initial_balance_amount: "0",
};

function codeStatus(code: RegistrationInvitationCodeView) {
  if (!code.enabled) {
    return { value: "disabled", label: "Disabled", variant: "destructive" as const };
  }
  if (code.expires_at && new Date(code.expires_at).getTime() <= Date.now()) {
    return { value: "expired", label: "Expired", variant: "warning" as const };
  }
  if (code.max_uses !== null && code.used_count >= code.max_uses) {
    return { value: "exhausted", label: "Exhausted", variant: "warning" as const };
  }
  return { value: "active", label: "Active", variant: "success" as const };
}

export function RegistrationInvitationCodeDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const detail = useRegistrationInvitationCode(id);
  const groups = useUserGroups();
  const create = useCreateRegistrationInvitationCode();
  const update = useUpdateRegistrationInvitationCode(id);
  const { t } = useI18n();
  const [createdSecret, setCreatedSecret] = useState<string | null>(null);
  const [createdId, setCreatedId] = useState<string | null>(null);

  const schema = baseSchema.superRefine((value, context) => {
    if (isNew) {
      const code = value.invitation_code.trim();
      if (code.length < 12) {
        context.addIssue({
          code: "custom",
          path: ["invitation_code"],
          message: "Invitation code must be at least 12 characters.",
        });
      } else if (code.length > 128) {
        context.addIssue({
          code: "custom",
          path: ["invitation_code"],
          message: "Invitation code must be at most 128 characters.",
        });
      } else if (/\s/.test(code)) {
        context.addIssue({
          code: "custom",
          path: ["invitation_code"],
          message: "Invitation code cannot contain whitespace.",
        });
      }
    }
    if (
      value.max_uses !== null &&
      detail.data &&
      value.max_uses < detail.data.data.used_count
    ) {
      context.addIssue({
        code: "custom",
        path: ["max_uses"],
        message: "Maximum uses cannot be below the current usage count.",
      });
    }
  });

  const form = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: empty,
  });

  useEffect(() => {
    if (detail.data) {
      form.reset({
        name: detail.data.data.name,
        invitation_code: "",
        max_uses: detail.data.data.max_uses,
        expires_at: formatDateTimeLocalInput(detail.data.data.expires_at),
        enabled: detail.data.data.enabled,
        user_group_id: detail.data.data.user_group_id,
        initial_balance_amount: detail.data.data.initial_balance_amount,
      });
    }
  }, [detail.data, form]);

  useEffect(() => {
    if (!isNew || form.getValues("user_group_id") || !groups.data) return;
    const defaultGroup =
      groups.data.find((group) => group.system_role === "user") ?? groups.data[0];
    if (defaultGroup) {
      form.setValue("user_group_id", defaultGroup.id, {
        shouldDirty: false,
        shouldValidate: true,
      });
    }
  }, [form, groups.data, isNew]);

  const code = detail.data?.data;
  const status = code ? codeStatus(code) : null;
  const groupName = code
    ? groups.data?.find((group) => group.id === code.user_group_id)?.name ??
      code.user_group_id
    : "";
  const pending = create.isPending || update.isPending;
  const fieldError = (name: keyof FormValues) => {
    const message = form.formState.errors[name]?.message;
    return typeof message === "string" ? t(message) : undefined;
  };

  const submit = form.handleSubmit(
    async (values) => {
      const common: RegistrationInvitationCodeUpdateInput = {
        name: values.name.trim(),
        max_uses: values.max_uses,
        expires_at: dateTimeLocalToIso(values.expires_at),
        enabled: values.enabled,
        user_group_id: values.user_group_id,
        initial_balance_amount: values.initial_balance_amount,
      };
      try {
        if (isNew) {
          const input: RegistrationInvitationCodeCreateInput = {
            ...common,
            invitation_code: values.invitation_code.trim(),
          };
          const result = await create.mutateAsync(input);
          setCreatedId(result.id);
          setCreatedSecret(result.invitation_code);
          toast.success(t("Registration code created"));
        } else {
          await update.mutateAsync({ input: common, ifMatch: detail.etag });
          toast.success(t("Registration code updated"));
        }
      } catch (error) {
        if (error instanceof ApiError && error.isConflict) {
          if (error.code === "registration_invitation_code_conflict") {
            toast.error(t("A registration code with this name or value already exists."));
          } else {
            toast.error(t("This registration code was changed elsewhere. Reloading."));
            await detail.refetch();
          }
        } else {
          toast.error(error instanceof Error ? error.message : t("Save failed"));
        }
      }
    },
    () => toast.error(t("Review the highlighted registration code fields.")),
  );

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New registration code") : code?.name ?? t("Registration code")}
        description={t(
          "Settings are evaluated atomically when each user registers and affect only future accounts.",
        )}
        backPath="/admin/registration-invitation-codes"
        backLabel={t("Back to registration codes")}
        isLoading={detail.isLoading || groups.isLoading}
        error={detail.error ?? groups.error}
        hasData={isNew || Boolean(code)}
        detailCard={
          !isNew && code && status ? (
            <Card>
              <CardHeader>
                <CardTitle>{code.name}</CardTitle>
                <CardDescription>
                  {t("Usage and current availability of this registration code.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
                  <DetailField
                    label={t("Status")}
                    value={
                      <StatusBadge
                        value={status.value}
                        label={t(status.label)}
                        variant={status.variant}
                      />
                    }
                  />
                  <DetailField
                    label={t("Uses")}
                    value={
                      code.max_uses === null
                        ? t("{used} / unlimited", { used: code.used_count })
                        : `${code.used_count} / ${code.max_uses}`
                    }
                  />
                  <DetailField label={t("User group")} value={groupName} />
                  <DetailField
                    label={t("Initial balance")}
                    value={formatUsd(code.initial_balance_amount)}
                  />
                  <DetailField label={t("Expires")} value={formatExpiry(code.expires_at)} />
                  <DetailField
                    label={t("Last used")}
                    value={formatDateTime(code.last_used_at)}
                  />
                </dl>
              </CardContent>
            </Card>
          ) : null
        }
        editCard={
          <Card>
            <CardHeader>
              <CardTitle>
                {isNew ? t("Create registration code") : t("Edit registration code")}
              </CardTitle>
              <CardDescription>
                {t("Choose who can register, how often, and with which starting balance.")}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form onSubmit={submit} className="flex flex-col gap-4">
                <FieldGroup>
                  <Field data-invalid={Boolean(fieldError("name"))}>
                    <FieldLabel htmlFor="registration_code_name">{t("Name")}</FieldLabel>
                    <Input
                      id="registration_code_name"
                      aria-invalid={Boolean(fieldError("name"))}
                      {...form.register("name")}
                    />
                    {fieldError("name") ? (
                      <FieldError>{fieldError("name")}</FieldError>
                    ) : null}
                  </Field>

                  {isNew ? (
                    <Field data-invalid={Boolean(fieldError("invitation_code"))}>
                      <FieldLabel htmlFor="registration_code_value">
                        {t("Invitation code")}
                      </FieldLabel>
                      <Input
                        id="registration_code_value"
                        autoComplete="off"
                        aria-invalid={Boolean(fieldError("invitation_code"))}
                        {...form.register("invitation_code")}
                      />
                      <FieldDescription>
                        {t(
                          "Use 12 to 128 case-sensitive characters without whitespace. The value cannot be recovered or changed after creation.",
                        )}
                      </FieldDescription>
                      {fieldError("invitation_code") ? (
                        <FieldError>{fieldError("invitation_code")}</FieldError>
                      ) : null}
                    </Field>
                  ) : (
                    <Alert>
                      <AlertTitle>{t("Invitation code is immutable")}</AlertTitle>
                      <AlertDescription>
                        {t(
                          "The gateway stores only a hash. To replace the value, create a new registration code and disable this one.",
                        )}
                      </AlertDescription>
                    </Alert>
                  )}

                  <Controller
                    control={form.control}
                    name="user_group_id"
                    render={({ field, fieldState }) => (
                      <Field data-invalid={fieldState.invalid}>
                        <FieldLabel htmlFor="registration_code_group">
                          {t("User group")}
                        </FieldLabel>
                        <Select value={field.value} onValueChange={field.onChange}>
                          <SelectTrigger
                            id="registration_code_group"
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
                        <FieldDescription>
                          {t("Future registrations are assigned to this group.")}
                        </FieldDescription>
                        {fieldState.error ? (
                          <FieldError>{t(fieldState.error.message ?? "")}</FieldError>
                        ) : null}
                      </Field>
                    )}
                  />

                  <Controller
                    control={form.control}
                    name="initial_balance_amount"
                    render={({ field, fieldState }) => (
                      <DecimalField
                        id="registration_code_initial_balance"
                        label={t("Initial balance")}
                        value={field.value}
                        onChange={field.onChange}
                        error={
                          fieldState.error?.message
                            ? t(fieldState.error.message)
                            : undefined
                        }
                        required
                        description={t(
                          "Non-negative USD balance assigned to each future account.",
                        )}
                      />
                    )}
                  />

                  <Controller
                    control={form.control}
                    name="max_uses"
                    render={({ field, fieldState }) => (
                      <NullableNumberField
                        id="registration_code_max_uses"
                        label={t("Maximum uses")}
                        value={field.value}
                        onChange={field.onChange}
                        error={
                          fieldState.error?.message
                            ? t(fieldState.error.message)
                            : undefined
                        }
                        description={t("Leave blank for unlimited registrations.")}
                      />
                    )}
                  />

                  <Field data-invalid={Boolean(fieldError("expires_at"))}>
                    <FieldLabel htmlFor="registration_code_expires_at">
                      {t("Expires")}
                    </FieldLabel>
                    <Input
                      id="registration_code_expires_at"
                      type="datetime-local"
                      aria-invalid={Boolean(fieldError("expires_at"))}
                      {...form.register("expires_at")}
                    />
                    <FieldDescription>
                      {t("Leave blank for no time-based expiry.")}
                    </FieldDescription>
                    {fieldError("expires_at") ? (
                      <FieldError>{fieldError("expires_at")}</FieldError>
                    ) : null}
                  </Field>

                  <Controller
                    control={form.control}
                    name="enabled"
                    render={({ field }) => (
                      <Field orientation="horizontal">
                        <FieldLabel htmlFor="registration_code_enabled">
                          {t("Enabled")}
                        </FieldLabel>
                        <Switch
                          id="registration_code_enabled"
                          checked={field.value}
                          onCheckedChange={(checked) => field.onChange(Boolean(checked))}
                        />
                      </Field>
                    )}
                  />
                </FieldGroup>
                <Button type="submit" className="self-start" disabled={pending}>
                  {pending ? <Spinner data-icon="inline-start" /> : null}
                  {isNew ? t("Create registration code") : t("Save registration code")}
                </Button>
              </form>
            </CardContent>
          </Card>
        }
      />
      <SecretOnceDialog
        open={Boolean(createdSecret)}
        onOpenChange={(open) => {
          if (open) return;
          setCreatedSecret(null);
          if (createdId) {
            navigate(`/admin/registration-invitation-codes/${createdId}`, {
              replace: true,
            });
          }
        }}
        title={t("Registration invitation code")}
        description={t(
          "Save the code now. Future detail views show only its settings and usage.",
        )}
        secret={createdSecret}
      />
    </>
  );
}
