import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { StringListField } from "@/components/shared/string-list-field";
import { DecimalField, NullableNumberField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useApiKeyPolicy,
  useCreateApiKeyPolicy,
  useUpdateApiKeyPolicy,
} from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type { ApiFormat, ApiKeyPolicyInput } from "@/api/types";
import { API_FORMATS, apiFormatLabel } from "@/lib/permissions";
import { formatRelative } from "@/lib/dates";
import { useI18n } from "@/app/i18n";

const schema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  allowed_api_formats: z.array(z.string()).min(1, "Pick at least one format."),
  permissions: z.array(z.string()),
  allowed_group_ids: z.array(z.string()).nullable(),
  requests_per_minute: z.number().int().positive().nullable(),
  max_concurrent_requests: z.number().int().positive().nullable(),
  quota_limit_amount: z.string().nullable(),
  max_active_keys: z.number().int().positive(),
  enabled: z.boolean(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  allowed_api_formats: [],
  permissions: ["proxy"],
  allowed_group_ids: null,
  requests_per_minute: null,
  max_concurrent_requests: null,
  quota_limit_amount: null,
  max_active_keys: 1,
  enabled: true,
};

export function ApiKeyPolicyDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useApiKeyPolicy(id);
  const create = useCreateApiKeyPolicy();
  const update = useUpdateApiKeyPolicy(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        allowed_api_formats: data.data.allowed_api_formats,
        permissions: data.data.permissions,
        allowed_group_ids: data.data.allowed_group_ids,
        requests_per_minute: data.data.requests_per_minute,
        max_concurrent_requests: data.data.max_concurrent_requests,
        quota_limit_amount: data.data.quota_limit_amount,
        max_active_keys: data.data.max_active_keys,
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    const input: ApiKeyPolicyInput = {
      name: parsed.data.name,
      allowed_api_formats: parsed.data.allowed_api_formats as ApiFormat[],
      permissions: parsed.data.permissions,
      allowed_group_ids: parsed.data.allowed_group_ids,
      requests_per_minute: parsed.data.requests_per_minute,
      max_concurrent_requests: parsed.data.max_concurrent_requests,
      quota_limit_amount: parsed.data.quota_limit_amount,
      max_active_keys: parsed.data.max_active_keys,
      enabled: parsed.data.enabled,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("Policy created"));
        navigate("/admin/api-key-policies", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Policy updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This policy was changed elsewhere. Reloading."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Save failed"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)?.message;
    return message ? t(message) : undefined;
  };

  return (
    <AdminDetailShell
      title={isNew ? t("New API key policy") : state.name || t("Policy")}
      description={t("Bounds the formats, permissions, rate limits, and quota of self-service keys.")}
      backPath="/admin/api-key-policies"
      backLabel={t("Back to policies")}
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Policy")}</CardTitle>
              <CardDescription>
                {t("Updated")} {formatRelative(data.data.updated_at)}.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField
                  label={t("Enabled")}
                  value={<StatusBadge value={data.data.enabled} />}
                />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? t("Create policy") : t("Edit policy")}</CardTitle>
            <CardDescription>
              {t("Users cannot raise these values when creating keys.")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup>
                <Field data-invalid={Boolean(fieldError("name"))}>
                  <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                  <Input
                    id="name"
                    value={state.name}
                    onChange={(event) => patch({ name: event.target.value })}
                    aria-invalid={Boolean(fieldError("name"))}
                  />
                  {fieldError("name") ? (
                    <FieldError>{fieldError("name")}</FieldError>
                  ) : null}
                </Field>
                <FieldSet>
                  <FieldLegend variant="label">{t("Allowed API formats")}</FieldLegend>
                  <FieldGroup data-slot="checkbox-group" className="gap-3">
                    {API_FORMATS.map((format) => (
                      <Field
                        key={format}
                        orientation="horizontal"
                        data-invalid={Boolean(fieldError("allowed_api_formats"))}
                      >
                        <Checkbox
                          id={`allowed_api_format_${format}`}
                          checked={state.allowed_api_formats.includes(format)}
                          aria-invalid={Boolean(fieldError("allowed_api_formats"))}
                          onCheckedChange={(checked) =>
                            patch({
                              allowed_api_formats: checked
                                ? [...state.allowed_api_formats, format]
                                : state.allowed_api_formats.filter((item) => item !== format),
                            })
                          }
                        />
                        <FieldLabel
                          htmlFor={`allowed_api_format_${format}`}
                          className="font-normal"
                        >
                          {apiFormatLabel(format)}
                        </FieldLabel>
                      </Field>
                    ))}
                  </FieldGroup>
                  {fieldError("allowed_api_formats") ? (
                    <FieldError>{fieldError("allowed_api_formats")}</FieldError>
                  ) : null}
                </FieldSet>
                <StringListField
                  label={t("Permissions")}
                  value={state.permissions}
                  onChange={(value) => patch({ permissions: value })}
                  placeholder="proxy, models.read"
                />
                <StringListField
                  label={t("Allowed group IDs")}
                  description={t(
                    "Restrict self-service keys to these channel groups. Empty means unrestricted.",
                  )}
                  value={state.allowed_group_ids ?? []}
                  onChange={(value) => patch({ allowed_group_ids: value.length ? value : null })}
                  placeholder={t("UUID per line")}
                />
                <NullableNumberField
                  label={t("Requests / minute")}
                  value={state.requests_per_minute}
                  onChange={(value) => patch({ requests_per_minute: value })}
                />
                <NullableNumberField
                  label={t("Max concurrent requests")}
                  value={state.max_concurrent_requests}
                  onChange={(value) => patch({ max_concurrent_requests: value })}
                />
                <DecimalField
                  label={t("Quota limit amount")}
                  value={state.quota_limit_amount}
                  onChange={(value) => patch({ quota_limit_amount: value || null })}
                />
                <Field data-invalid={Boolean(fieldError("max_active_keys"))}>
                  <FieldLabel htmlFor="max_active_keys">{t("Max active keys")}</FieldLabel>
                  <Input
                    id="max_active_keys"
                    type="number"
                    min={1}
                    value={state.max_active_keys}
                    onChange={(event) =>
                      patch({ max_active_keys: Math.max(1, Number(event.target.value) || 1) })
                    }
                    aria-invalid={Boolean(fieldError("max_active_keys"))}
                  />
                  {fieldError("max_active_keys") ? (
                    <FieldError>{fieldError("max_active_keys")}</FieldError>
                  ) : null}
                </Field>
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="policy_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="policy_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create policy") : t("Save policy")}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
