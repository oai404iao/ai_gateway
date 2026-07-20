import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { StringListField } from "@/components/shared/string-list-field";
import { NullableNumberField } from "@/components/shared/decimal-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useChannel,
  useChannelGroups,
  useConfigTemplates,
  useCreateChannel,
  useProxies,
  useUpdateChannel,
} from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type {
  ApiFormat,
  ChannelCreateInput,
  ChannelInput,
  UpstreamAuthKind,
} from "@/api/types";
import { UPSTREAM_AUTH_KINDS, apiFormatLabel, upstreamAuthKindLabel } from "@/lib/permissions";

const schema = z.object({
  channel_group_id: z.string().min(1, "Pick a channel group."),
  api_format: z.enum(["open_ai_chat_completions", "open_ai_responses"]),
  name: z.string().min(1, "Name is required.").max(100),
  base_url: z.string().min(1, "Base URL is required."),
  enabled: z.boolean(),
  weight: z.number().int().min(0),
  proxy_id: z.string().nullable(),
  config_template_id: z.string().nullable(),
  override_document: z.string(),
  connect_timeout_ms: z.number().int().positive().nullable(),
  response_header_timeout_ms: z.number().int().positive().nullable(),
  stream_idle_timeout_ms: z.number().int().positive().nullable(),
  upstream_auth_kind: z.enum(["bearer", "header"]),
  upstream_auth_header_name: z.string().nullable(),
  upstream_api_key: z.string(),
  available_models: z.array(z.string()),
  health_check: z.string(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  channel_group_id: "",
  api_format: "open_ai_chat_completions",
  name: "",
  base_url: "",
  enabled: true,
  weight: 100,
  proxy_id: null,
  config_template_id: null,
  override_document: "{}",
  connect_timeout_ms: null,
  response_header_timeout_ms: null,
  stream_idle_timeout_ms: null,
  upstream_auth_kind: "bearer",
  upstream_auth_header_name: null,
  upstream_api_key: "",
  available_models: [],
  health_check: "{}",
};

export function ChannelDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useChannel(id);
  const create = useCreateChannel();
  const update = useUpdateChannel(id);
  const groups = useChannelGroups();
  const proxies = useProxies();
  const templates = useConfigTemplates();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        channel_group_id: data.data.channel_group_id,
        api_format: data.data.api_format,
        name: data.data.name,
        base_url: data.data.base_url,
        enabled: data.data.enabled,
        weight: data.data.weight,
        proxy_id: data.data.proxy_id,
        config_template_id: data.data.config_template_id,
        override_document: "{}",
        connect_timeout_ms: data.data.connect_timeout_ms,
        response_header_timeout_ms: data.data.response_header_timeout_ms,
        stream_idle_timeout_ms: data.data.stream_idle_timeout_ms,
        upstream_auth_kind: data.data.upstream_auth_kind,
        upstream_auth_header_name: data.data.upstream_auth_header_name,
        upstream_api_key: "",
        available_models: data.data.available_models,
        health_check: "{}",
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  // When the group changes, align the channel format to the group's format.
  const selectedGroup = useMemo(
    () => groups.data?.find((group) => group.id === state.channel_group_id),
    [groups.data, state.channel_group_id],
  );

  const submit = async () => {
    let overrideDocument: unknown = {};
    let healthCheck: unknown = {};
    try {
      overrideDocument = state.override_document.trim()
        ? JSON.parse(state.override_document)
        : {};
      healthCheck = state.health_check.trim() ? JSON.parse(state.health_check) : {};
    } catch {
      toast.error("Override document or health check is not valid JSON.");
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
        const input: ChannelCreateInput = {
          channel_group_id: parsed.data.channel_group_id,
          api_format: parsed.data.api_format as ApiFormat,
          name: parsed.data.name,
          base_url: parsed.data.base_url,
          enabled: parsed.data.enabled,
          weight: parsed.data.weight,
          proxy_id: parsed.data.proxy_id,
          config_template_id: parsed.data.config_template_id,
          override_document: overrideDocument,
          connect_timeout_ms: parsed.data.connect_timeout_ms,
          response_header_timeout_ms: parsed.data.response_header_timeout_ms,
          stream_idle_timeout_ms: parsed.data.stream_idle_timeout_ms,
          upstream_auth_kind: parsed.data.upstream_auth_kind as UpstreamAuthKind,
          upstream_auth_header_name: parsed.data.upstream_auth_header_name,
          upstream_api_key: parsed.data.upstream_api_key || null,
          available_models: parsed.data.available_models,
          health_check: healthCheck,
        };
        await create.mutateAsync(input);
        toast.success("Channel created");
        navigate("/admin/routing/channels", { replace: true });
      } else {
        // On edit, omit upstream_api_key when blank to keep the current secret.
        const input: ChannelInput = {
          channel_group_id: parsed.data.channel_group_id,
          api_format: parsed.data.api_format as ApiFormat,
          name: parsed.data.name,
          base_url: parsed.data.base_url,
          enabled: parsed.data.enabled,
          weight: parsed.data.weight,
          proxy_id: parsed.data.proxy_id,
          config_template_id: parsed.data.config_template_id,
          override_document: overrideDocument,
          connect_timeout_ms: parsed.data.connect_timeout_ms,
          response_header_timeout_ms: parsed.data.response_header_timeout_ms,
          stream_idle_timeout_ms: parsed.data.stream_idle_timeout_ms,
          upstream_auth_kind: parsed.data.upstream_auth_kind as UpstreamAuthKind,
          upstream_auth_header_name: parsed.data.upstream_auth_header_name,
          available_models: parsed.data.available_models,
          health_check: healthCheck,
        };
        if (parsed.data.upstream_api_key !== "") {
          input.upstream_api_key = parsed.data.upstream_api_key;
        }
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success("Channel updated");
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error("This channel was changed elsewhere. Reloading.");
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
      title={isNew ? "New channel" : state.name || "Channel"}
      description="An upstream endpoint with weight, timeouts, and credential injection."
      backPath="/admin/routing/channels"
      backLabel="Back to channels"
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.name}</CardTitle>
              <CardDescription className="font-mono">{data.data.base_url}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <dt className="text-xs uppercase text-muted-foreground">Enabled</dt>
                <dd>
                  <StatusBadge value={data.data.enabled} />
                </dd>
                <dt className="text-xs uppercase text-muted-foreground">Auto-disabled</dt>
                <dd>
                  <StatusBadge value={data.data.auto_disabled} />
                </dd>
                <dt className="text-xs uppercase text-muted-foreground">Credential configured</dt>
                <dd>{data.data.upstream_credential_configured ? "yes" : "no"}</dd>
                <dt className="text-xs uppercase text-muted-foreground">Weight</dt>
                <dd>{data.data.weight}</dd>
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? "Create channel" : "Edit channel"}</CardTitle>
            <CardDescription>
              The channel format must match its group's format.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup>
                <Field>
                  <FieldLabel>Channel group</FieldLabel>
                  <Select
                    value={state.channel_group_id || "__none__"}
                    onValueChange={(value) => {
                      const group = groups.data?.find((item) => item.id === value);
                      patch({
                        channel_group_id: value === "__none__" ? "" : value,
                        api_format: (group?.api_format ?? state.api_format) as ApiFormat,
                      });
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Pick a group" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="__none__">None</SelectItem>
                        {groups.data?.map((group) => (
                          <SelectItem key={group.id} value={group.id}>
                            {group.name} ({apiFormatLabel(group.api_format)})
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  {fieldError("channel_group_id") ? (
                    <FieldError>{fieldError("channel_group_id")}</FieldError>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel>API format</FieldLabel>
                  <Input value={apiFormatLabel(selectedGroup?.api_format ?? state.api_format)} disabled />
                </Field>
                <Field>
                  <FieldLabel htmlFor="name">Name</FieldLabel>
                  <Input id="name" value={state.name} onChange={(event) => patch({ name: event.target.value })} />
                  {fieldError("name") ? <FieldError>{fieldError("name")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="base_url">Base URL</FieldLabel>
                  <Input
                    id="base_url"
                    value={state.base_url}
                    onChange={(event) => patch({ base_url: event.target.value })}
                    placeholder="https://api.upstream.com"
                  />
                  {fieldError("base_url") ? <FieldError>{fieldError("base_url")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="weight">Weight</FieldLabel>
                  <Input
                    id="weight"
                    type="number"
                    min={0}
                    value={state.weight}
                    onChange={(event) => patch({ weight: Number(event.target.value) || 0 })}
                  />
                </Field>
                <Field>
                  <FieldLabel>Proxy</FieldLabel>
                  <Select
                    value={state.proxy_id ?? "__none__"}
                    onValueChange={(value) => patch({ proxy_id: value === "__none__" ? null : value })}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="__none__">None</SelectItem>
                        {proxies.data?.map((proxy) => (
                          <SelectItem key={proxy.id} value={proxy.id}>
                            {proxy.name}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field>
                  <FieldLabel>Config template</FieldLabel>
                  <Select
                    value={state.config_template_id ?? "__none__"}
                    onValueChange={(value) =>
                      patch({ config_template_id: value === "__none__" ? null : value })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="__none__">None</SelectItem>
                        {templates.data?.map((template) => (
                          <SelectItem key={template.id} value={template.id}>
                            {template.name}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field>
                  <FieldLabel>Upstream auth kind</FieldLabel>
                  <Select
                    value={state.upstream_auth_kind}
                    onValueChange={(value) =>
                      patch({ upstream_auth_kind: value as UpstreamAuthKind })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {UPSTREAM_AUTH_KINDS.map((kind) => (
                          <SelectItem key={kind} value={kind}>
                            {upstreamAuthKindLabel(kind)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                {state.upstream_auth_kind === "header" ? (
                  <Field>
                    <FieldLabel htmlFor="header_name">Header name</FieldLabel>
                    <Input
                      id="header_name"
                      value={state.upstream_auth_header_name ?? ""}
                      onChange={(event) =>
                        patch({ upstream_auth_header_name: event.target.value || null })
                      }
                      placeholder="x-api-key"
                    />
                  </Field>
                ) : null}
                <Field>
                  <FieldLabel htmlFor="upstream_api_key">
                    Upstream API key{" "}
                    {!isNew ? (
                      <span className="text-xs text-muted-foreground">
                        (leave blank to keep current)
                      </span>
                    ) : null}
                  </FieldLabel>
                  <Input
                    id="upstream_api_key"
                    value={state.upstream_api_key}
                    onChange={(event) => patch({ upstream_api_key: event.target.value })}
                    autoComplete="off"
                  />
                </Field>
                <StringListField
                  label="Available upstream models"
                  value={state.available_models}
                  onChange={(value) => patch({ available_models: value })}
                  placeholder="One upstream model id per line"
                />
                <NullableNumberField
                  label="Connect timeout (ms)"
                  value={state.connect_timeout_ms}
                  onChange={(value) => patch({ connect_timeout_ms: value })}
                />
                <NullableNumberField
                  label="Response header timeout (ms)"
                  value={state.response_header_timeout_ms}
                  onChange={(value) => patch({ response_header_timeout_ms: value })}
                />
                <NullableNumberField
                  label="Stream idle timeout (ms)"
                  value={state.stream_idle_timeout_ms}
                  onChange={(value) => patch({ stream_idle_timeout_ms: value })}
                />
                <Field>
                  <FieldLabel htmlFor="override_document">Override document (JSON)</FieldLabel>
                  <Textarea
                    id="override_document"
                    rows={5}
                    className="font-mono text-xs"
                    value={state.override_document}
                    onChange={(event) => patch({ override_document: event.target.value })}
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="health_check">Health check (JSON)</FieldLabel>
                  <Textarea
                    id="health_check"
                    rows={3}
                    className="font-mono text-xs"
                    value={state.health_check}
                    onChange={(event) => patch({ health_check: event.target.value })}
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
                {isNew ? "Create channel" : "Save channel"}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
