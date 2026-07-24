import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { TransformDocumentEditor } from "@/features/admin/transforms/transform-document-editor";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useConfigTemplate,
  useCreateConfigTemplate,
  useUpdateConfigTemplate,
} from "@/features/admin/api";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ConfigTemplateCreateInput, ConfigTemplateInput } from "@/api/types";
import { useI18n } from "@/app/i18n";

const schema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  description: z.string().nullable(),
  document: z.string(),
  enabled: z.boolean(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  description: null,
  document: "{}",
  enabled: true,
};

export function ConfigTemplateDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useConfigTemplate(id);
  const create = useCreateConfigTemplate();
  const update = useUpdateConfigTemplate(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [documentValidation, setDocumentValidation] = useState<string | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        description: data.data.description,
        document: JSON.stringify(data.data.document, null, 2) ?? "{}",
        enabled: data.data.enabled,
      });
      setDocumentValidation(null);
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const submit = async () => {
    if (documentValidation) {
      toast.error(t(documentValidation));
      return;
    }
    let document: unknown | undefined;
    try {
      if (state.document.trim()) {
        document = JSON.parse(state.document);
      } else if (isNew) {
        document = {};
      }
    } catch {
      toast.error(t("Template document is not valid JSON."));
      return;
    }
    if (
      document !== undefined &&
      (typeof document !== "object" || document === null || Array.isArray(document))
    ) {
      toast.error(t("Template document must be a JSON object."));
      return;
    }
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    try {
      if (isNew) {
        const input: ConfigTemplateCreateInput = {
          name: parsed.data.name,
          description: parsed.data.description,
          document: document ?? {},
          enabled: parsed.data.enabled,
        };
        await create.mutateAsync(input);
        toast.success(t("Template created"));
        navigate("/admin/transforms/templates", { replace: true });
      } else {
        const input: ConfigTemplateInput = {
          name: parsed.data.name,
          description: parsed.data.description,
          enabled: parsed.data.enabled,
        };
        if (document !== undefined) {
          input.document = document;
        }
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Template updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This template was changed elsewhere. Reloading."));
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
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
      title={isNew ? t("New template") : state.name || t("Template")}
      description={t("A reusable constrained transform document.")}
      backPath="/admin/transforms/templates"
      backLabel={t("Back to templates")}
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.name}</CardTitle>
              <CardDescription>{data.data.description ?? "—"}</CardDescription>
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
            <CardTitle>{isNew ? t("Create template") : t("Edit template")}</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup className="grid gap-5 xl:grid-cols-2">
                <Field data-invalid={Boolean(fieldError("name"))}>
                  <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                  <Input
                    id="name"
                    value={state.name}
                    onChange={(event) => patch({ name: event.target.value })}
                    aria-invalid={Boolean(fieldError("name"))}
                  />
                  {fieldError("name") ? <FieldError>{fieldError("name")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="description">{t("Description")}</FieldLabel>
                  <Input
                    id="description"
                    value={state.description ?? ""}
                    onChange={(event) => patch({ description: event.target.value || null })}
                  />
                </Field>
                <Field className="xl:col-span-2">
                  <FieldLabel>{t("Document (JSON)")}</FieldLabel>
                  <FieldDescription>
                    {t("Constrained transform document.")}
                  </FieldDescription>
                  <TransformDocumentEditor
                    value={state.document}
                    onChange={(document) => patch({ document })}
                    defaultApiFormat={data?.data.api_format ?? undefined}
                    onVisualValidationChange={setDocumentValidation}
                  />
                </Field>
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="template_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="template_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create template") : t("Save template")}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
