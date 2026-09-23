import { useEffect, useState } from "react";
import { useParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useCreateRoutingGroup,
  useDeleteRoutingGroup,
  useRoutingGroup,
  useUpdateRoutingGroup,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import type { RoutingGroupInput } from "@/api/types";
import { useReturnPath, withReturnTo } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

const schema = z.object({
  name: z.string().trim().min(1).max(100),
  enabled: z.boolean(),
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = { name: "", enabled: true };

export function GroupDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const returnTo = useReturnPath("/admin/routing/channels?view=groups");
  const { t } = useI18n();
  const query = useRoutingGroup(id);
  const create = useCreateRoutingGroup();
  const update = useUpdateRoutingGroup(id);
  const remove = useDeleteRoutingGroup(id);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  const group = query.data?.data;
  const busy = create.isPending || update.isPending || remove.isPending;
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(busy, form.formState.isDirty);

  useEffect(() => {
    if (group) {
      form.reset({
        name: group.name,
        enabled: group.enabled,
      });
    }
  }, [group, form]);

  const submit = form.handleSubmit(async (values) => {
    const input: RoutingGroupInput = values;
    try {
      if (isNew) {
        const result = await create.mutateAsync(input);
        markSaved();
        navigate(withReturnTo(`/admin/routing/groups/${result.id}`, returnTo), { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
      }
      form.reset(values);
      markSaved();
      toast.success(t("Group saved"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This group was changed elsewhere. Reloading."));
        await query.refetch();
      } else {
        toast.error(t(controlPlaneMutationErrorMessage(error, "Could not save routing group.")));
      }
    }
  });

  const confirmDelete = async () => {
    try {
      await remove.mutateAsync({ ifMatch: query.etag });
      toast.success(t("Group deleted"));
      markSaved();
      navigate(returnTo, { replace: true });
    } catch (error) {
      toast.error(t(controlPlaneMutationErrorMessage(error, "Could not delete routing group.")));
    }
  };

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New group") : group?.name ?? t("Channel group")}
        description={t(
          "Groups organize logical channels. Deleting a group requires every member channel to be removed first.",
        )}
        backPath={returnTo}
        isLoading={!isNew && query.isLoading}
        error={query.error}
        hasData={isNew || Boolean(group)}
        navigationGuard={navigationGuard}
        editCard={
          <Card>
            <CardHeader>
              <CardTitle>{t("Group settings")}</CardTitle>
              <CardDescription>
                {t("Saving a group never creates channels, capabilities, routes, or API key access.")}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form onSubmit={submit} className="flex flex-col gap-5">
                <FieldGroup>
                  <Field data-invalid={Boolean(form.formState.errors.name)}>
                    <FieldLabel htmlFor="group-name">{t("Name")}</FieldLabel>
                    <Input
                      id="group-name"
                      {...form.register("name")}
                      aria-invalid={Boolean(form.formState.errors.name)}
                    />
                    <FieldError errors={[form.formState.errors.name]} />
                  </Field>
                  <Field orientation="horizontal">
                    <FieldLabel htmlFor="group-enabled">{t("Enabled")}</FieldLabel>
                    <Switch
                      id="group-enabled"
                      checked={form.watch("enabled")}
                      onCheckedChange={(value) =>
                        form.setValue("enabled", value, { shouldDirty: true })
                      }
                    />
                  </Field>
                </FieldGroup>
                <Button type="submit" disabled={busy}>
                  {t("Save group")}
                </Button>
              </form>
            </CardContent>
          </Card>
        }
        dangerZone={
          isNew ? undefined : (
            <div className="flex flex-col gap-3">
              <p className="text-sm text-muted-foreground">
                {t("Any member channel, including disabled channels, blocks deletion.")}
              </p>
              <Button
                type="button"
                variant="destructive"
                className="self-start"
                disabled={busy}
                onClick={() => setConfirmingDelete(true)}
              >
                {t("Delete group")}
              </Button>
            </div>
          )
        }
      />
      <ConfirmDialog
        open={confirmingDelete}
        onOpenChange={setConfirmingDelete}
        title={t("Delete this routing group?")}
        description={t("The group is soft-deleted after all member channels are removed.")}
        destructive
        confirmDisabled={busy}
        onConfirm={confirmDelete}
      />
    </>
  );
}
