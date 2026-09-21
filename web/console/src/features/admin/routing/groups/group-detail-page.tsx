import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
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
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useCreateRoutingGroup,
  useDeleteRoutingGroup,
  useRoutingGroup,
  useUpdateRoutingGroup,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import type { RoutingGroupInput } from "@/api/types";

const schema = z.object({
  name: z.string().trim().min(1).max(100),
  enabled: z.boolean(),
  sharing_only: z.boolean(),
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = { name: "", enabled: true, sharing_only: false };

export function GroupDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { t } = useI18n();
  const query = useRoutingGroup(id);
  const create = useCreateRoutingGroup();
  const update = useUpdateRoutingGroup(id);
  const remove = useDeleteRoutingGroup(id);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  const group = query.data?.data;
  const busy = create.isPending || update.isPending || remove.isPending;

  useEffect(() => {
    if (group) {
      form.reset({
        name: group.name,
        enabled: group.enabled,
        sharing_only: group.sharing_only,
      });
    }
  }, [group, form]);

  const submit = form.handleSubmit(async (values) => {
    const input: RoutingGroupInput = values;
    try {
      if (isNew) {
        const result = await create.mutateAsync(input);
        navigate(`/admin/routing/groups/${result.id}`, { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: query.etag });
      }
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
      navigate("/admin/routing/groups");
    } catch (error) {
      toast.error(t(controlPlaneMutationErrorMessage(error, "Could not delete routing group.")));
    }
  };

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New group") : group?.name ?? t("Routing group")}
        description={t(
          "Groups organize logical channels. Deleting a group requires every member channel to be removed first.",
        )}
        backPath="/admin/routing/groups"
        isLoading={!isNew && query.isLoading}
        error={query.error}
        hasData={isNew || Boolean(group)}
        saving={busy}
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
                  <Field>
                    <FieldLabel htmlFor="group-sharing">{t("Sharing only")}</FieldLabel>
                    <Switch
                      id="group-sharing"
                      checked={form.watch("sharing_only")}
                      onCheckedChange={(value) =>
                        form.setValue("sharing_only", value, { shouldDirty: true })
                      }
                    />
                    <FieldDescription>
                      {t(
                        "Codex-only. Rejected unless every live member channel uses the Codex OAuth connector.",
                      )}
                    </FieldDescription>
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
