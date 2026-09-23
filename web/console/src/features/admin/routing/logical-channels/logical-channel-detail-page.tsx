import { useEffect, useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CapabilitiesPage } from "../capabilities/capabilities-page";
import { CapabilityDetailPage } from "../capabilities/capability-detail-page";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useCreateLogicalChannel,
  useDeleteLogicalChannel,
  useLogicalChannel,
  useRoutingGroups,
  useUpdateLogicalChannel,
  useUpstreamAccesses,
  useUpstreamCredentials,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import type { LogicalChannelInput } from "@/api/types";
import { useReturnPath, withReturnTo } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

const NONE = "__none__";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const schema = z.object({
  group_id: z.string().regex(UUID, "invalid"),
  access_id: z.string().regex(UUID, "invalid"),
  credential_id: z.string().refine((value) => value === NONE || UUID.test(value), "invalid"),
  name: z.string().trim().min(1).max(100),
  enabled: z.boolean(),
  sharing_only: z.boolean(),
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = {
  group_id: "",
  access_id: "",
  credential_id: NONE,
  name: "",
  enabled: true,
  sharing_only: false,
};

export function LogicalChannelDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const returnTo = useReturnPath("/admin/routing/channels");
  const [params, setParams] = useSearchParams();
  const view = !isNew && params.get("view") === "capabilities" ? "capabilities" : "settings";
  const capabilityId = params.get("capability");
  const capabilityListParams = new URLSearchParams(params);
  capabilityListParams.delete("capability");
  const editingCapability = view === "capabilities" && Boolean(capabilityId);
  const { t } = useI18n();
  const query = useLogicalChannel(id);
  const groups = useRoutingGroups();
  const accesses = useUpstreamAccesses();
  const credentials = useUpstreamCredentials();
  const create = useCreateLogicalChannel();
  const update = useUpdateLogicalChannel(id);
  const remove = useDeleteLogicalChannel(id);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  const channel = query.data?.data;
  const busy = create.isPending || update.isPending || remove.isPending;
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(busy, form.formState.isDirty);
  const access = accesses.data?.find((item) => item.id === form.watch("access_id"));
  const compatibleCredentials = credentials.data?.filter((credential) =>
    credential.id === form.watch("credential_id") ||
    (credential.connector_kind === access?.connector_kind &&
      credential.allowed_base_urls.some((url) => url.replace(/\/+$/, "") === access?.base_url.replace(/\/+$/, ""))),
  );

  useEffect(() => {
    if (channel) {
      form.reset({
        group_id: channel.group_id,
        access_id: channel.access_id,
        credential_id: channel.credential_id ?? NONE,
        name: channel.name,
        enabled: channel.enabled,
        sharing_only: channel.sharing_only,
      });
    }
  }, [channel, form, view]);

  const submit = form.handleSubmit(async (values) => {
    const input: LogicalChannelInput = {
      group_id: values.group_id,
      access_id: values.access_id,
      credential_id: values.credential_id === NONE ? null : values.credential_id,
      name: values.name,
      enabled: values.enabled,
      sharing_only: values.sharing_only,
    };
    try {
      if (isNew) {
        const result = await create.mutateAsync(input);
        markSaved();
        navigate(withReturnTo(`/admin/routing/logical-channels/${result.id}`, returnTo), { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
      }
      form.reset(values);
      markSaved();
      toast.success(t("Channel saved"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This channel was changed elsewhere. Reloading."));
        await query.refetch();
      } else {
        toast.error(t(controlPlaneMutationErrorMessage(error, "Could not save logical channel.")));
      }
    }
  });

  const confirmDelete = async () => {
    try {
      await remove.mutateAsync({ ifMatch: query.etag });
      toast.success(t("Channel deleted"));
      markSaved();
      navigate(returnTo, { replace: true });
    } catch (error) {
      toast.error(t(controlPlaneMutationErrorMessage(error, "Could not delete logical channel.")));
    }
  };

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New channel") : channel?.name ?? t("Logical channel")}
        description={t(
          "A channel binds authentication to one access. Shared accesses and credentials are never deleted with it.",
        )}
        backPath={editingCapability
          ? `/admin/routing/logical-channels/${id}?${capabilityListParams}` : returnTo}
        backLabel={editingCapability ? t("Back to capabilities") : undefined}
        isLoading={!isNew && query.isLoading}
        error={query.error}
        hasData={isNew || Boolean(channel)}
        navigationGuard={navigationGuard}
        editCard={
          <Tabs value={view} onValueChange={(value) => setParams((current) => {
            const next = new URLSearchParams(current);
            next.set("view", String(value));
            next.delete("capability");
            next.delete("page");
            return next;
          })}>
            <TabsList>
              <TabsTrigger value="settings">{t("Channel settings")}</TabsTrigger>
              <TabsTrigger value="capabilities" disabled={isNew}>{t("Channel capabilities")}</TabsTrigger>
            </TabsList>
            <TabsContent value="settings">
          <Card>
            <CardHeader>
              <CardTitle>{t("Channel settings")}</CardTitle>
              <CardDescription>
                {t("Credentials are reusable identities. Rebinding changes connection identity without touching routes.")}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form onSubmit={submit} className="flex flex-col gap-5">
                <FieldGroup>
                  <Field data-invalid={Boolean(form.formState.errors.name)}>
                    <FieldLabel htmlFor="channel-name">{t("Name")}</FieldLabel>
                    <Input
                      id="channel-name"
                      {...form.register("name")}
                      aria-invalid={Boolean(form.formState.errors.name)}
                    />
                    <FieldError errors={[form.formState.errors.name]} />
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.group_id)}>
                    <FieldLabel htmlFor="channel-group">{t("Channel group")}</FieldLabel>
                    <Select
                      value={form.watch("group_id") || NONE}
                      onValueChange={(value) =>
                        form.setValue("group_id", value === NONE ? "" : value, { shouldDirty: true })
                      }
                    >
                      <SelectTrigger
                        id="channel-group"
                        aria-invalid={Boolean(form.formState.errors.group_id)}
                      >
                        <SelectValue placeholder={t("Pick a group")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value={NONE}>{t("Choose a group")}</SelectItem>
                          {groups.data?.map((group) => (
                            <SelectItem key={group.id} value={group.id}>
                              {group.name}
                              {!group.enabled ? ` · ${t("Disabled")}` : ""}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldError errors={[form.formState.errors.group_id]} />
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.access_id)}>
                    <FieldLabel htmlFor="channel-access">{t("Upstream access")}</FieldLabel>
                    <Select
                      value={form.watch("access_id") || NONE}
                      onValueChange={(value) => {
                        form.setValue("access_id", value === NONE ? "" : value, { shouldDirty: true });
                        form.setValue("credential_id", NONE, { shouldDirty: true });
                        if (accesses.data?.find((item) => item.id === value)?.connector_kind !== "codex")
                          form.setValue("sharing_only", false, { shouldDirty: true });
                      }}
                    >
                      <SelectTrigger
                        id="channel-access"
                        aria-invalid={Boolean(form.formState.errors.access_id)}
                      >
                        <SelectValue placeholder={t("Pick an access")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value={NONE}>{t("Choose an access")}</SelectItem>
                          {accesses.data?.map((access) => (
                            <SelectItem key={access.id} value={access.id}>
                              {access.name} ({access.connector_kind === "codex" ? "Codex" : t("General")})
                              {!access.enabled ? ` · ${t("Disabled")}` : ""}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldError errors={[form.formState.errors.access_id]} />
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.credential_id)}>
                    <FieldLabel htmlFor="channel-credential">{t("Credential")}</FieldLabel>
                    <Select
                      value={form.watch("credential_id")}
                      onValueChange={(value) =>
                        form.setValue("credential_id", value, { shouldDirty: true })
                      }
                    >
                      <SelectTrigger
                        id="channel-credential"
                        aria-invalid={Boolean(form.formState.errors.credential_id)}
                      >
                        <SelectValue placeholder={t("Choose a credential")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem value={NONE}>{access?.connector_kind === "codex" ? t("Choose a credential") : t("No authentication")}</SelectItem>
                          {compatibleCredentials?.map((credential) => (
                            <SelectItem
                              key={credential.id}
                              value={credential.id}
                            >
                              {credential.name}{!credential.enabled ? ` · ${t("Disabled")}` : ""}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                    <FieldError errors={[form.formState.errors.credential_id]} />
                  </Field>
                  {access?.connector_kind === "codex" && <Field>
                    <FieldLabel htmlFor="channel-sharing">{t("Sharing only")}</FieldLabel>
                    <Switch id="channel-sharing" checked={form.watch("sharing_only")}
                      onCheckedChange={(value) => form.setValue("sharing_only", value, { shouldDirty: true })} />
                    <FieldDescription>{t("Only seated sharing members can use this channel. Other channels for the same account cannot bypass sharing protection.")}</FieldDescription>
                  </Field>}
                  <Field orientation="horizontal">
                    <FieldLabel htmlFor="channel-enabled">{t("Enabled")}</FieldLabel>
                    <Switch
                      id="channel-enabled"
                      checked={form.watch("enabled")}
                      onCheckedChange={(value) =>
                        form.setValue("enabled", value, { shouldDirty: true })
                      }
                    />
                  </Field>
                </FieldGroup>
                <Button type="submit" disabled={busy}>
                  {t("Save channel")}
                </Button>
              </form>
            </CardContent>
          </Card>
            </TabsContent>
            {!isNew && <TabsContent value="capabilities" className="flex flex-col gap-5">
              {capabilityId ? <CapabilityDetailPage key={`${id}:${capabilityId}`}
                embedded channelId={id} capabilityId={capabilityId} /> : <CapabilitiesPage channelId={id} />}
            </TabsContent>}
          </Tabs>
        }
        dangerZone={
          isNew || view !== "settings" ? undefined : (
            <div className="flex flex-col gap-3">
              <p className="text-sm text-muted-foreground">
                {t("Delete this channel's capabilities first. Their routing candidates are removed automatically.")}
              </p>
              <Button
                type="button"
                variant="destructive"
                className="self-start"
                disabled={busy}
                onClick={() => setConfirmingDelete(true)}
              >
                {t("Delete channel")}
              </Button>
            </div>
          )
        }
      />
      <ConfirmDialog
        open={confirmingDelete}
        onOpenChange={setConfirmingDelete}
        title={t("Delete this logical channel?")}
        description={t("The channel is soft-deleted. Shared accesses and credentials are kept.")}
        destructive
        confirmDisabled={busy}
        onConfirm={confirmDelete}
      />
    </>
  );
}
