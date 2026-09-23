import { useEffect } from "react";
import { useParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import { useCreateUpstreamAccess, useProxies, useUpdateUpstreamAccess, useUpstreamAccess } from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { useReturnPath, withReturnTo } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

const timeout = z.string().regex(/^(?:[1-9][0-9]*)?$/)
  .refine((value) => value === "" || Number(value) <= 2147483647);
const schema = z.object({
  name: z.string().trim().min(1).max(100),
  connector_kind: z.enum(["general", "codex"]),
  base_url: z.string().trim().url(),
  proxy_id: z.string(),
  connect_timeout_ms: timeout,
  response_header_timeout_ms: timeout,
  stream_idle_timeout_ms: timeout,
  enabled: z.boolean(),
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = {
  name: "", connector_kind: "general", base_url: "", proxy_id: "__none__",
  connect_timeout_ms: "", response_header_timeout_ms: "", stream_idle_timeout_ms: "",
  enabled: false,
};
const timeoutFields = [
  ["connect_timeout_ms", "Connect timeout (ms)"],
  ["response_header_timeout_ms", "Response header timeout (ms)"],
  ["stream_idle_timeout_ms", "Stream idle timeout (ms)"],
] as const;
const optionalTimeout = (value: string) => value === "" ? null : Number(value);

export function AccessDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const returnTo = useReturnPath("/admin/routing/accesses");
  const { t } = useI18n();
  const query = useUpstreamAccess(id);
  const proxies = useProxies();
  const create = useCreateUpstreamAccess();
  const update = useUpdateUpstreamAccess(id);
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  const access = query.data?.data;
  const busy = create.isPending || update.isPending;
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(busy, form.formState.isDirty);
  useEffect(() => {
    if (access) form.reset({
      name: access.name, connector_kind: access.connector_kind, base_url: access.base_url,
      proxy_id: access.proxy_id ?? "__none__", enabled: access.enabled,
      connect_timeout_ms: access.connect_timeout_ms?.toString() ?? "",
      response_header_timeout_ms: access.response_header_timeout_ms?.toString() ?? "",
      stream_idle_timeout_ms: access.stream_idle_timeout_ms?.toString() ?? "",
    });
  }, [access, form]);
  const submit = form.handleSubmit(async (values) => {
    const input = {
      name: values.name, connector_kind: values.connector_kind, base_url: values.base_url,
      proxy_id: values.proxy_id === "__none__" ? null : values.proxy_id,
      connect_timeout_ms: optionalTimeout(values.connect_timeout_ms),
      response_header_timeout_ms: optionalTimeout(values.response_header_timeout_ms),
      stream_idle_timeout_ms: optionalTimeout(values.stream_idle_timeout_ms),
      enabled: values.enabled,
    };
    try {
      if (isNew) {
        const result = await create.mutateAsync(input);
        markSaved();
        navigate(withReturnTo(`/admin/routing/accesses/${result.id}`, returnTo), { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
      }
      form.reset(values);
      markSaved();
      toast.success(t("Access saved"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This access was changed elsewhere. Reloading."));
        await query.refetch();
      } else {
        toast.error(t(controlPlaneMutationErrorMessage(error, "Could not save upstream access.")));
      }
    }
  });
  return <AdminDetailShell
    title={isNew ? t("New access") : access?.name ?? t("Upstream access")}
    description={t("Network changes affect all referencing channels and must satisfy every credential scope.")}
    backPath={returnTo}
    isLoading={!isNew && query.isLoading} error={query.error}
    hasData={isNew || Boolean(access)} navigationGuard={navigationGuard}
    editCard={<Card>
      <CardHeader>
        <CardTitle>{t("Access settings")}</CardTitle>
        <CardDescription>{t("Creating an access does not create channels, enable capabilities or grant API key access.")}</CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="flex flex-col gap-5">
          <FieldGroup>
            <Field data-invalid={Boolean(form.formState.errors.name)}>
              <FieldLabel htmlFor="access-name">{t("Name")}</FieldLabel>
              <Input id="access-name" {...form.register("name")} aria-invalid={Boolean(form.formState.errors.name)} />
              <FieldError errors={[form.formState.errors.name]} />
            </Field>
            <Field data-disabled={!isNew}>
              <FieldLabel htmlFor="access-connector">{t("Connector")}</FieldLabel>
              <Select value={form.watch("connector_kind")} disabled={!isNew}
                onValueChange={(value) => form.setValue("connector_kind", value as FormValues["connector_kind"], { shouldDirty: true })}>
                <SelectTrigger id="access-connector"><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>
                  <SelectItem value="general">{t("General")}</SelectItem>
                  <SelectItem value="codex">Codex</SelectItem>
                </SelectGroup></SelectContent>
              </Select>
            </Field>
            <Field data-invalid={Boolean(form.formState.errors.base_url)}>
              <FieldLabel htmlFor="access-url">Base URL</FieldLabel>
              <Input id="access-url" {...form.register("base_url")} aria-invalid={Boolean(form.formState.errors.base_url)} />
              <FieldError errors={[form.formState.errors.base_url]} />
            </Field>
            <Field>
              <FieldLabel htmlFor="access-proxy">{t("Proxy")}</FieldLabel>
              <Select value={form.watch("proxy_id")} onValueChange={(value) => {
                if (value !== null) form.setValue("proxy_id", value, { shouldDirty: true });
              }}>
                <SelectTrigger id="access-proxy"><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>
                  <SelectItem value="__none__">{t("No proxy")}</SelectItem>
                  {proxies.data?.map((proxy) => <SelectItem key={proxy.id} value={proxy.id} disabled={!proxy.enabled}>{proxy.name}</SelectItem>)}
                </SelectGroup></SelectContent>
              </Select>
              {proxies.error ? <FieldDescription>{t("Could not load proxies.")}</FieldDescription> : null}
            </Field>
            {timeoutFields.map(([field, label]) => <Field key={field} data-invalid={Boolean(form.formState.errors[field])}>
              <FieldLabel htmlFor={`access-${field}`}>{t(label)}</FieldLabel>
              <Input id={`access-${field}`} inputMode="numeric" {...form.register(field)} aria-invalid={Boolean(form.formState.errors[field])} />
              <FieldDescription>{t("Leave blank to use the operation default.")}</FieldDescription>
              <FieldError errors={[form.formState.errors[field]]} />
            </Field>)}
            <Field orientation="horizontal">
              <FieldLabel htmlFor="access-enabled">{t("Enabled")}</FieldLabel>
              <Switch id="access-enabled" checked={form.watch("enabled")}
                onCheckedChange={(value) => form.setValue("enabled", value, { shouldDirty: true })} />
            </Field>
          </FieldGroup>
          <Button type="submit" disabled={busy}>{t("Save access")}</Button>
        </form>
      </CardContent>
    </Card>}
  />;
}
