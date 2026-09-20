import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
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
import { StringListField } from "@/components/shared/string-list-field";
import { ApiKeyValue } from "@/components/shared/api-key-value";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useChannels, useCreateUpstreamCredential, useDeleteUpstreamCredential,
  useUpdateUpstreamCredential, useUpstreamCredential,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";

const schema = z.object({
  name: z.string().trim().min(1).max(100),
  kind: z.enum(["bearer", "header"]),
  header_name: z.string(),
  secret: z.string(),
  allowed_base_urls: z.array(z.string().url()).min(1),
  enabled: z.boolean(),
}).superRefine((value, context) => {
  if (value.kind === "header" && !value.header_name.trim()) {
    context.addIssue({ code: "custom", path: ["header_name"], message: "A custom header name is required." });
  }
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = { name: "", kind: "bearer", header_name: "", secret: "", allowed_base_urls: [], enabled: true };

export function CredentialDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { t } = useI18n();
  const query = useUpstreamCredential(id);
  const channels = useChannels();
  const create = useCreateUpstreamCredential();
  const update = useUpdateUpstreamCredential(id);
  const remove = useDeleteUpstreamCredential(id);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  const credential = query.data?.data;
  const managed = credential?.provider_managed ?? false;
  const busy = create.isPending || update.isPending || remove.isPending;
  useEffect(() => {
    if (credential && !credential.provider_managed) {
      form.reset({
        name: credential.name, kind: credential.kind as "bearer" | "header",
        header_name: credential.header_name ?? "", secret: "",
        allowed_base_urls: credential.allowed_base_urls, enabled: credential.enabled,
      });
    }
  }, [credential, form]);

  const fail = async (error: unknown) => {
    if (error instanceof ApiError && error.isConflict) {
      toast.error(t("This credential was changed elsewhere or is still in use. Reloading."));
      await query.refetch();
    } else {
      toast.error(t(controlPlaneMutationErrorMessage(error, "Could not save upstream credential.")));
    }
  };
  const submit = form.handleSubmit(async (values) => {
    if ((isNew || values.secret !== "") && !values.secret.trim()) {
      form.setError("secret", { message: t("A credential secret is required.") });
      return;
    }
    const input = {
      name: values.name, kind: values.kind,
      header_name: values.kind === "header" ? values.header_name.trim() : null,
      allowed_base_urls: values.allowed_base_urls, enabled: values.enabled,
    };
    try {
      if (isNew) {
        const result = await create.mutateAsync({ ...input, secret: values.secret });
        navigate(`/admin/routing/upstream-credentials/${result.id}`, { replace: true });
      } else {
        await update.mutateAsync({ input: { ...input, ...(values.secret !== "" ? { secret: values.secret } : {}) }, ifMatch: query.etag });
      }
      form.setValue("secret", "");
      toast.success(t("Credential saved"));
    } catch (error) { await fail(error); }
  });

  return <AdminDetailShell
    title={isNew ? t("New credential") : credential?.name ?? t("Upstream credential")}
    description={t("Rotating or disabling this identity affects every referencing channel.")}
    backPath="/admin/routing/upstream-credentials"
    isLoading={!isNew && query.isLoading} error={query.error}
    hasData={isNew || Boolean(credential)} saving={busy}
    detailCard={credential ? <Card>
      <CardHeader><CardTitle>{t("Referencing channels")}</CardTitle>
        <CardDescription>{t("Disabled channels also retain their credential binding.")}</CardDescription></CardHeader>
      <CardContent className="flex flex-col gap-3">
        {credential.channel_ids.length === 0 ? <p>{t("No referencing channels.")}</p> : credential.channel_ids.map((channelId) => {
          const channel = channels.data?.find((value) => value.id === channelId);
          const path = managed && channel
            ? `/admin/providers/codex-oauth/${channel.channel_group_id}`
            : `/admin/routing/channels/${channelId}`;
          return <Link key={channelId} to={path}>{channel?.name ?? channelId}</Link>;
        })}
        {managed ? <p>{t("This identity is managed by the Codex connector. Use its provider page to change or delete it.")}</p> : null}
        {credential.secret ? <ApiKeyValue value={credential.secret} /> : null}
      </CardContent>
    </Card> : null}
    editCard={!managed ? <Card>
      <CardHeader><CardTitle>{t("Credential settings")}</CardTitle>
        <CardDescription>{t("Allowed Base URLs are exact scopes, not host or path prefixes.")}</CardDescription></CardHeader>
      <CardContent>
        <form onSubmit={submit} className="flex flex-col gap-5">
          <FieldGroup>
            <Field data-invalid={Boolean(form.formState.errors.name)}>
              <FieldLabel htmlFor="credential-name">{t("Name")}</FieldLabel>
              <Input id="credential-name" {...form.register("name")} aria-invalid={Boolean(form.formState.errors.name)} />
              <FieldError errors={[form.formState.errors.name]} />
            </Field>
            <Field>
              <FieldLabel htmlFor="credential-kind">{t("Authentication type")}</FieldLabel>
              <Select value={form.watch("kind")} disabled={!isNew}
                onValueChange={(value) => form.setValue("kind", value as FormValues["kind"], { shouldDirty: true })}>
                <SelectTrigger id="credential-kind"><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>
                  <SelectItem value="bearer">Bearer</SelectItem><SelectItem value="header">{t("Custom header")}</SelectItem>
                </SelectGroup></SelectContent>
              </Select>
            </Field>
            {form.watch("kind") === "header" ? <Field data-invalid={Boolean(form.formState.errors.header_name)}>
              <FieldLabel htmlFor="credential-header">{t("Header name")}</FieldLabel>
              <Input id="credential-header" {...form.register("header_name")} aria-invalid={Boolean(form.formState.errors.header_name)} />
              <FieldError errors={[form.formState.errors.header_name]} />
            </Field> : null}
            <Field data-invalid={Boolean(form.formState.errors.secret)}>
              <FieldLabel htmlFor="credential-secret">{t("Credential secret")}</FieldLabel>
              <Input id="credential-secret" type="password" autoComplete="new-password" {...form.register("secret")} aria-invalid={Boolean(form.formState.errors.secret)} />
              {!isNew ? <FieldDescription>{t("Leave blank to preserve the current secret.")}</FieldDescription> : null}
              <FieldError errors={[form.formState.errors.secret]} />
            </Field>
            <StringListField id="credential-targets" label={t("Allowed Base URLs")} value={form.watch("allowed_base_urls")}
              onChange={(value) => form.setValue("allowed_base_urls", value, { shouldDirty: true, shouldValidate: true })}
              error={form.formState.errors.allowed_base_urls ? t("Enter one or more valid Base URLs.") : undefined}
              placeholder="https://api.example.test" />
            <Field orientation="horizontal">
              <FieldLabel htmlFor="credential-enabled">{t("Enabled")}</FieldLabel>
              <Switch id="credential-enabled" checked={form.watch("enabled")}
                onCheckedChange={(value) => form.setValue("enabled", value, { shouldDirty: true })} />
            </Field>
          </FieldGroup>
          <Button type="submit" disabled={busy}>{t("Save credential")}</Button>
        </form>
      </CardContent>
    </Card> : null}
    dangerZone={!isNew && !managed ? <>
      <Button variant="destructive" disabled={busy || Boolean(credential?.channel_ids.length)} onClick={() => setConfirmDelete(true)}>{t("Delete credential")}</Button>
      <ConfirmDialog open={confirmDelete} onOpenChange={setConfirmDelete}
        title={t("Delete credential?")} description={t("The secret will be erased permanently. Referencing channels must be unbound first.")}
        confirmLabel={t("Delete credential")} destructive
        onConfirm={() => {
          void remove.mutateAsync(query.etag).then(() => navigate("/admin/routing/upstream-credentials")).catch(fail);
        }} />
    </> : null}
  />;
}
