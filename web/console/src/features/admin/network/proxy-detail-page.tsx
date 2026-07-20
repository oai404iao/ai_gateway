import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { StringListField } from "@/components/shared/string-list-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { useCreateProxy, useProxy, useUpdateProxy } from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type { ProxyCreateInput, ProxyInput } from "@/api/types";

const schema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  proxy_url: z.string().min(1, "Proxy URL is required."),
  username: z.string().nullable(),
  password: z.string().nullable(),
  no_proxy_hosts: z.array(z.string()),
  enabled: z.boolean(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  proxy_url: "",
  username: null,
  password: null,
  no_proxy_hosts: [],
  enabled: true,
};

export function ProxyDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useProxy(id);
  const create = useCreateProxy();
  const update = useUpdateProxy(id);
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        proxy_url: data.data.proxy_url,
        username: null,
        password: null,
        no_proxy_hosts: data.data.no_proxy_hosts,
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
    try {
      if (isNew) {
        const input: ProxyCreateInput = {
          name: parsed.data.name,
          proxy_url: parsed.data.proxy_url,
          username: parsed.data.username,
          password: parsed.data.password,
          no_proxy_hosts: parsed.data.no_proxy_hosts,
          enabled: parsed.data.enabled,
        };
        await create.mutateAsync(input);
        toast.success("Proxy created");
        navigate("/admin/network/proxies", { replace: true });
      } else {
        // On edit, omit blank credentials to keep current values.
        const input: ProxyInput = {
          name: parsed.data.name,
          proxy_url: parsed.data.proxy_url,
          no_proxy_hosts: parsed.data.no_proxy_hosts,
          enabled: parsed.data.enabled,
        };
        if (parsed.data.username !== null) input.username = parsed.data.username;
        if (parsed.data.password !== null) input.password = parsed.data.password;
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success("Proxy updated");
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error("This proxy was changed elsewhere. Reloading.");
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
      title={isNew ? "New proxy" : state.name || "Proxy"}
      description="An egress proxy shared by upstream clients."
      backPath="/admin/network/proxies"
      backLabel="Back to proxies"
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.name}</CardTitle>
              <CardDescription className="font-mono">{data.data.proxy_url}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <dt className="text-xs uppercase text-muted-foreground">Enabled</dt>
                <dd>
                  <StatusBadge value={data.data.enabled} />
                </dd>
                <dt className="text-xs uppercase text-muted-foreground">Credential configured</dt>
                <dd>{data.data.credential_configured ? "yes" : "no"}</dd>
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? "Create proxy" : "Edit proxy"}</CardTitle>
            <CardDescription>
              {!isNew ? "Leave credential fields blank to keep current values." : null}
            </CardDescription>
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
                  <FieldLabel htmlFor="proxy_url">Proxy URL</FieldLabel>
                  <Input
                    id="proxy_url"
                    value={state.proxy_url}
                    onChange={(event) => patch({ proxy_url: event.target.value })}
                    placeholder="https://user:pass@host:1080"
                  />
                  {fieldError("proxy_url") ? <FieldError>{fieldError("proxy_url")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="username">Username</FieldLabel>
                  <Input
                    id="username"
                    value={state.username ?? ""}
                    onChange={(event) => patch({ username: event.target.value || null })}
                    autoComplete="off"
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="password">Password</FieldLabel>
                  <Input
                    id="password"
                    type="password"
                    value={state.password ?? ""}
                    onChange={(event) => patch({ password: event.target.value || null })}
                    autoComplete="new-password"
                  />
                </Field>
                <StringListField
                  label="No-proxy hosts"
                  description="Hosts that bypass the proxy."
                  value={state.no_proxy_hosts}
                  onChange={(value) => patch({ no_proxy_hosts: value })}
                  placeholder="example.com, .internal"
                />
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
                {isNew ? "Create proxy" : "Save proxy"}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
