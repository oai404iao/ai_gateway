import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DecimalField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { useCreateModel, useModel, useUpdateModel } from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type { ModelInput } from "@/api/types";
import { formatDateTime } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";

const schema = z.object({
  source_model_id: z.string().min(1, "Source model id is required."),
  display_name: z.string().min(1, "Display name is required."),
  provider_name: z.string().nullable(),
  enabled: z.boolean(),
  price_unit_tokens: z.number().int().positive(),
  input_unit_price: z.string().min(1),
  cached_input_unit_price: z.string().min(1),
  cache_write_unit_price: z.string().min(1),
  output_unit_price: z.string().min(1),
  price_effective_at: z.string().min(1),
  source_payload: z.string(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  source_model_id: "",
  display_name: "",
  provider_name: null,
  enabled: true,
  price_unit_tokens: 1_000_000,
  input_unit_price: "0",
  cached_input_unit_price: "0",
  cache_write_unit_price: "0",
  output_unit_price: "0",
  price_effective_at: new Date().toISOString(),
  source_payload: "{}",
};

function toLocalInput(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (n: number) => (n < 10 ? `0${n}` : String(n));
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

function fromLocalInput(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toISOString();
}

export function ModelDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useModel(id);
  const create = useCreateModel();
  const update = useUpdateModel(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        source_model_id: data.data.source_model_id,
        display_name: data.data.display_name,
        provider_name: data.data.provider_name,
        enabled: data.data.enabled,
        price_unit_tokens: data.data.price_unit_tokens,
        input_unit_price: data.data.input_unit_price,
        cached_input_unit_price: data.data.cached_input_unit_price,
        cache_write_unit_price: data.data.cache_write_unit_price,
        output_unit_price: data.data.output_unit_price,
        price_effective_at: data.data.price_effective_at,
        source_payload: "{}",
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const submit = async () => {
    let payload: unknown = undefined;
    try {
      payload = state.source_payload.trim() ? JSON.parse(state.source_payload) : {};
    } catch {
      toast.error(t("Source payload is not valid JSON."));
      return;
    }
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    const input: ModelInput = {
      source_model_id: parsed.data.source_model_id,
      display_name: parsed.data.display_name,
      provider_name: parsed.data.provider_name,
      enabled: parsed.data.enabled,
      price_unit_tokens: parsed.data.price_unit_tokens,
      input_unit_price: parsed.data.input_unit_price,
      cached_input_unit_price: parsed.data.cached_input_unit_price,
      cache_write_unit_price: parsed.data.cache_write_unit_price,
      output_unit_price: parsed.data.output_unit_price,
      price_effective_at: fromLocalInput(parsed.data.price_effective_at),
      source_payload: payload,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("Upstream model created"));
        navigate("/admin/models", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Upstream model updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This upstream model was changed elsewhere. Reloading."));
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
      title={isNew ? t("New upstream model") : state.display_name || t("Upstream model")}
      description={t("An upstream model identifier with its USD billing price.")}
      backPath="/admin/models"
      backLabel={t("Back to upstream models")}
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.display_name}</CardTitle>
              <CardDescription className="font-mono">{data.data.source_model_id}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField
                  label={t("Enabled")}
                  value={<StatusBadge value={data.data.enabled} />}
                />
                <DetailField label={t("Provider")} value={data.data.provider_name ?? "—"} />
                <DetailField
                  label={t("Input price")}
                  value={formatDecimal(data.data.input_unit_price)}
                />
                <DetailField
                  label={t("Output price")}
                  value={formatDecimal(data.data.output_unit_price)}
                />
                <DetailField
                  label={t("Effective")}
                  value={formatDateTime(data.data.price_effective_at)}
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
              {isNew ? t("Create upstream model") : t("Edit upstream model")}
            </CardTitle>
            <CardDescription>
              {t("USD prices are per the configured price unit tokens.")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup>
                <Field data-invalid={Boolean(fieldError("source_model_id"))}>
                  <FieldLabel htmlFor="source_model_id">{t("Source model id")}</FieldLabel>
                  <Input
                    id="source_model_id"
                    value={state.source_model_id}
                    onChange={(event) => patch({ source_model_id: event.target.value })}
                    aria-invalid={Boolean(fieldError("source_model_id"))}
                  />
                  {fieldError("source_model_id") ? (
                    <FieldError>{fieldError("source_model_id")}</FieldError>
                  ) : null}
                </Field>
                <Field data-invalid={Boolean(fieldError("display_name"))}>
                  <FieldLabel htmlFor="display_name">{t("Display name")}</FieldLabel>
                  <Input
                    id="display_name"
                    value={state.display_name}
                    onChange={(event) => patch({ display_name: event.target.value })}
                    aria-invalid={Boolean(fieldError("display_name"))}
                  />
                  {fieldError("display_name") ? (
                    <FieldError>{fieldError("display_name")}</FieldError>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="provider_name">{t("Provider name")}</FieldLabel>
                  <Input
                    id="provider_name"
                    value={state.provider_name ?? ""}
                    onChange={(event) => patch({ provider_name: event.target.value || null })}
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="price_unit_tokens">{t("Price unit tokens")}</FieldLabel>
                  <Input
                    id="price_unit_tokens"
                    type="number"
                    min={1}
                    value={state.price_unit_tokens}
                    onChange={(event) =>
                      patch({ price_unit_tokens: Math.max(1, Number(event.target.value) || 1) })
                    }
                  />
                </Field>
                <DecimalField
                  label={t("Input unit price")}
                  value={state.input_unit_price}
                  onChange={(value) => patch({ input_unit_price: value })}
                  error={fieldError("input_unit_price")}
                  required
                />
                <DecimalField
                  label={t("Cached input unit price")}
                  value={state.cached_input_unit_price}
                  onChange={(value) => patch({ cached_input_unit_price: value })}
                  error={fieldError("cached_input_unit_price")}
                  required
                />
                <DecimalField
                  label={t("Cache write unit price")}
                  value={state.cache_write_unit_price}
                  onChange={(value) => patch({ cache_write_unit_price: value })}
                  error={fieldError("cache_write_unit_price")}
                  required
                />
                <DecimalField
                  label={t("Output unit price")}
                  value={state.output_unit_price}
                  onChange={(value) => patch({ output_unit_price: value })}
                  error={fieldError("output_unit_price")}
                  required
                />
                <Field>
                  <FieldLabel htmlFor="price_effective_at">{t("Price effective at")}</FieldLabel>
                  <Input
                    id="price_effective_at"
                    type="datetime-local"
                    value={toLocalInput(state.price_effective_at)}
                    onChange={(event) => patch({ price_effective_at: fromLocalInput(event.target.value) })}
                  />
                </Field>
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="model_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="model_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="source_payload">
                    {t("Source payload (JSON, optional)")}
                  </FieldLabel>
                  <Textarea
                    id="source_payload"
                    rows={4}
                    className="font-mono text-xs"
                    value={state.source_payload}
                    onChange={(event) => patch({ source_payload: event.target.value })}
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create upstream model") : t("Save upstream model")}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
