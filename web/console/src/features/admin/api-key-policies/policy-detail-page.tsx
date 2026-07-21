import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { StringListField } from "@/components/shared/string-list-field";
import { DecimalField, NullableNumberField } from "@/components/shared/decimal-field";
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
                <dt className="text-xs uppercase text-muted-foreground">{t("Enabled")}</dt>
                <dd>
                  <StatusBadge value={data.data.enabled} />
                </dd>
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
                <Field>
                  <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                  <Input
                    id="name"
                    value={state.name}
                    onChange={(event) => patch({ name: event.target.value })}
                  />
                  {fieldError("name") ? (
                    <p className="text-sm text-destructive">{fieldError("name")}</p>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel>{t("Allowed API formats")}</FieldLabel>
                  <div className="flex flex-col gap-2">
                    {API_FORMATS.map((format) => (
                      <label key={format} className="flex items-center gap-2 text-sm">
                        <Checkbox
                          checked={state.allowed_api_formats.includes(format)}
                          onCheckedChange={(checked) =>
                            patch({
                              allowed_api_formats: checked
                                ? [...state.allowed_api_formats, format]
                                : state.allowed_api_formats.filter((item) => item !== format),
                            })
                          }
                        />
                        {apiFormatLabel(format)}
                      </label>
                    ))}
                  </div>
                  {fieldError("allowed_api_formats") ? (
                    <p className="text-sm text-destructive">
                      {fieldError("allowed_api_formats")}
                    </p>
                  ) : null}
                </Field>
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
                <Field>
                  <FieldLabel htmlFor="max_active_keys">{t("Max active keys")}</FieldLabel>
                  <Input
                    id="max_active_keys"
                    type="number"
                    min={1}
                    value={state.max_active_keys}
                    onChange={(event) =>
                      patch({ max_active_keys: Math.max(1, Number(event.target.value) || 1) })
                    }
                  />
                  {fieldError("max_active_keys") ? (
                    <p className="text-sm text-destructive">{fieldError("max_active_keys")}</p>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel>{t("Enabled")}</FieldLabel>
                  <Switch
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
