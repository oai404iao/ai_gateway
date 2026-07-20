import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useChannelGroups,
  useChannels,
  useCreateModelRule,
  useModelRule,
  useModels,
  useUpdateModelRule,
} from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type { ApiFormat, ModelRuleInput } from "@/api/types";
import { API_FORMATS, apiFormatLabel } from "@/lib/permissions";

const schema = z.object({
  client_model: z.string().min(1, "Client model is required."),
  api_format: z.enum(["open_ai_chat_completions", "open_ai_responses"]),
  model_id: z.string().min(1, "Pick a priced model."),
  upstream_model: z.string().min(1, "Upstream model is required."),
  description: z.string().nullable(),
  channel_group_ids: z.array(z.string()),
  channel_ids: z.array(z.string()),
  enabled: z.boolean(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  client_model: "",
  api_format: "open_ai_chat_completions",
  model_id: "",
  upstream_model: "",
  description: null,
  channel_group_ids: [],
  channel_ids: [],
  enabled: true,
};

export function ModelRuleDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useModelRule(id);
  const create = useCreateModelRule();
  const update = useUpdateModelRule(id);
  const models = useModels();
  const groups = useChannelGroups();
  const channels = useChannels();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        client_model: data.data.client_model,
        api_format: data.data.api_format,
        model_id: data.data.model_id,
        upstream_model: data.data.upstream_model,
        description: data.data.description,
        channel_group_ids: data.data.channel_group_ids,
        channel_ids: data.data.channel_ids,
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const eligibleGroups = useMemo(
    () => groups.data?.filter((group) => group.api_format === state.api_format) ?? [],
    [groups.data, state.api_format],
  );
  const eligibleChannels = useMemo(
    () => channels.data?.filter((channel) => channel.api_format === state.api_format) ?? [],
    [channels.data, state.api_format],
  );

  const toggle = (key: "channel_group_ids" | "channel_ids", value: string) => {
    setState((prev) => ({
      ...prev,
      [key]: prev[key].includes(value)
        ? prev[key].filter((item) => item !== value)
        : [...prev[key], value],
    }));
  };

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    const input: ModelRuleInput = {
      client_model: parsed.data.client_model,
      api_format: parsed.data.api_format as ApiFormat,
      model_id: parsed.data.model_id,
      upstream_model: parsed.data.upstream_model,
      description: parsed.data.description,
      channel_group_ids: parsed.data.channel_group_ids,
      channel_ids: parsed.data.channel_ids,
      enabled: parsed.data.enabled,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success("Model rule created");
        navigate("/admin/routing/model-rules", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success("Model rule updated");
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error("This rule was changed elsewhere. Reloading.");
      } else {
        toast.error(error instanceof Error ? error.message : "Save failed");
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) =>
    validation?.issues.find((issue) => issue.path.join(".") === path)?.message;

  return (
    <AdminDetailShell
      title={isNew ? "New model rule" : state.client_model || "Model rule"}
      description="Routes a client model and API format to a priced model and channels."
      backPath="/admin/routing/model-rules"
      backLabel="Back to rules"
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.client_model}</CardTitle>
              <CardDescription>{apiFormatLabel(data.data.api_format)}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <dt className="text-xs uppercase text-muted-foreground">Upstream model</dt>
                <dd className="font-mono text-xs">{data.data.upstream_model}</dd>
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
            <CardTitle>{isNew ? "Create rule" : "Edit rule"}</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="client_model">Client model</FieldLabel>
                  <Input
                    id="client_model"
                    value={state.client_model}
                    onChange={(event) => patch({ client_model: event.target.value })}
                  />
                  {fieldError("client_model") ? (
                    <FieldError>{fieldError("client_model")}</FieldError>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel>API format</FieldLabel>
                  <Select
                    value={state.api_format}
                    onValueChange={(value) =>
                      patch({ api_format: value as ApiFormat, channel_group_ids: [], channel_ids: [] })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {API_FORMATS.map((format) => (
                        <SelectItem key={format} value={format}>
                          {apiFormatLabel(format)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field>
                  <FieldLabel>Priced model</FieldLabel>
                  <Select
                    value={state.model_id || "__none__"}
                    onValueChange={(value) => patch({ model_id: value === "__none__" ? "" : value })}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Pick a model" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__none__">None</SelectItem>
                      {models.data?.map((model) => (
                        <SelectItem key={model.id} value={model.id}>
                          {model.display_name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  {fieldError("model_id") ? <FieldError>{fieldError("model_id")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="upstream_model">Upstream model</FieldLabel>
                  <Input
                    id="upstream_model"
                    value={state.upstream_model}
                    onChange={(event) => patch({ upstream_model: event.target.value })}
                  />
                  {fieldError("upstream_model") ? (
                    <FieldError>{fieldError("upstream_model")}</FieldError>
                  ) : null}
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
                  <FieldLabel>Channel groups ({eligibleGroups.length})</FieldLabel>
                  <div className="flex flex-col gap-2">
                    {eligibleGroups.map((group) => (
                      <label key={group.id} className="flex items-center gap-2 text-sm">
                        <Checkbox
                          checked={state.channel_group_ids.includes(group.id)}
                          onCheckedChange={() => toggle("channel_group_ids", group.id)}
                        />
                        {group.name} (priority {group.priority})
                      </label>
                    ))}
                    {eligibleGroups.length === 0 ? (
                      <span className="text-xs text-muted-foreground">
                        No groups for this format.
                      </span>
                    ) : null}
                  </div>
                </Field>
                <Field>
                  <FieldLabel>Channels ({eligibleChannels.length})</FieldLabel>
                  <div className="flex flex-col gap-2">
                    {eligibleChannels.map((channel) => (
                      <label key={channel.id} className="flex items-center gap-2 text-sm">
                        <Checkbox
                          checked={state.channel_ids.includes(channel.id)}
                          onCheckedChange={() => toggle("channel_ids", channel.id)}
                        />
                        {channel.name}
                      </label>
                    ))}
                    {eligibleChannels.length === 0 ? (
                      <span className="text-xs text-muted-foreground">
                        No channels for this format.
                      </span>
                    ) : null}
                  </div>
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
                {isNew ? "Create rule" : "Save rule"}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
