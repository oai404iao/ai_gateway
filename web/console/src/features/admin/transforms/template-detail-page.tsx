import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useConfigTemplate,
  useCreateConfigTemplate,
  useUpdateConfigTemplate,
} from "@/features/admin/api";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ConfigTemplateCreateInput, ConfigTemplateInput } from "@/api/types";

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
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        description: data.data.description,
        document: "",
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const submit = async () => {
    let document: unknown | undefined;
    try {
      if (state.document.trim()) {
        document = JSON.parse(state.document);
      } else if (isNew) {
        document = {};
      }
    } catch {
      toast.error("Template document is not valid JSON.");
      return;
    }
    if (
      document !== undefined &&
      (typeof document !== "object" || document === null || Array.isArray(document))
    ) {
      toast.error("Template document must be a JSON object.");
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
        toast.success("Template created");
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
        toast.success("Template updated");
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error("This template was changed elsewhere. Reloading.");
      } else {
        toast.error(controlPlaneMutationErrorMessage(error));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) =>
    validation?.issues.find((issue) => issue.path.join(".") === path)?.message;

  return (
    <AdminDetailShell
      title={isNew ? "New template" : state.name || "Template"}
      description="A reusable constrained transform document."
      backPath="/admin/transforms/templates"
      backLabel="Back to templates"
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
                <dt className="text-xs uppercase text-muted-foreground">Enabled</dt>
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
            <CardTitle>{isNew ? "Create template" : "Edit template"}</CardTitle>
            {!isNew ? (
              <CardDescription>
                The current document is redacted. Leave the JSON blank to preserve it; enter {"{}"} to
                clear it.
              </CardDescription>
            ) : null}
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="name">Name</FieldLabel>
                  <Input id="name" value={state.name} onChange={(event) => patch({ name: event.target.value })} />
                  {fieldError("name") ? <FieldError>{fieldError("name")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="description">Description</FieldLabel>
                  <Input
                    id="description"
                    value={state.description ?? ""}
                    onChange={(event) => patch({ description: event.target.value || null })}
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="document">Document (JSON)</FieldLabel>
                  <FieldDescription>
                    {isNew ? "Constrained transform document." : "Optional replacement document."}
                  </FieldDescription>
                  <Textarea
                    id="document"
                    rows={10}
                    className="font-mono text-xs"
                    value={state.document}
                    onChange={(event) => patch({ document: event.target.value })}
                  />
                </Field>
                <Field>
                  <FieldLabel>Enabled</FieldLabel>
                  <Switch
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? "Create template" : "Save template"}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
