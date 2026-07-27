import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import type { UserGroupInput } from "@/api/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { DetailField } from "@/components/shared/detail-field";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import {
  useApiKeyPolicies,
  useCreateUserGroup,
  useDeleteUserGroup,
  useUpdateUserGroup,
  useUserGroup,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { roleLabel } from "@/lib/permissions";

const schema = z.object({
  name: z.string().trim().min(1, "Name is required.").max(100),
  description: z.string().max(500),
  default_api_key_policy_id: z.string(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  description: "",
  default_api_key_policy_id: "",
};

export function UserGroupDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const detail = useUserGroup(id);
  const policies = useApiKeyPolicies();
  const create = useCreateUserGroup();
  const update = useUpdateUserGroup(id);
  const remove = useDeleteUserGroup(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);

  useEffect(() => {
    if (detail.data) {
      setState({
        name: detail.data.data.name,
        description: detail.data.data.description ?? "",
        default_api_key_policy_id:
          detail.data.data.default_api_key_policy_id ?? "",
      });
    }
  }, [detail.data]);

  const group = detail.data?.data;
  const patch = (partial: Partial<FormState>) =>
    setState((current) => ({ ...current, ...partial }));
  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)
      ?.message;
    return message ? t(message) : undefined;
  };
  const policyName = group?.default_api_key_policy_id
    ? policies.data?.find((policy) => policy.id === group.default_api_key_policy_id)
        ?.name ?? group.default_api_key_policy_id
    : t("None");

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    const input: UserGroupInput = {
      name: parsed.data.name,
      description: parsed.data.description || null,
      default_api_key_policy_id:
        parsed.data.default_api_key_policy_id || null,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("User group created"));
        navigate("/admin/user-groups", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: detail.etag });
        toast.success(t("User group updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This user group was changed elsewhere. Reloading."));
        await detail.refetch();
      } else {
        toast.error(error instanceof Error ? error.message : t("Save failed"));
      }
    }
  };

  const deleteGroup = async () => {
    setDeleteOpen(false);
    try {
      await remove.mutateAsync({ ifMatch: detail.etag });
      toast.success(t("User group deleted"));
      navigate("/admin/user-groups", { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.code === "user_group_in_use") {
        toast.error(t("Move every member out of this group before deleting it."));
      } else if (
        error instanceof ApiError &&
        error.code === "protected_user_group"
      ) {
        toast.error(t("Built-in default groups cannot be deleted."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Delete failed"));
      }
    }
  };

  const pending = create.isPending || update.isPending || remove.isPending;

  return (
    <>
      <AdminDetailShell
        title={isNew ? t("New user group") : state.name || t("User group")}
        description={t("Group defaults apply when a user has no policy override.")}
        backPath="/admin/user-groups"
        backLabel={t("Back to user groups")}
        isLoading={detail.isLoading || policies.isLoading}
        error={detail.error ?? policies.error}
        hasData={isNew || Boolean(group)}
        detailCard={
          !isNew && group ? (
            <Card>
              <CardHeader>
                <CardTitle>{group.name}</CardTitle>
                <CardDescription>
                  {group.system_role
                    ? t("Protected default group for {role} accounts.", {
                        role: roleLabel(group.system_role),
                      })
                    : t("Custom user group")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                  <DetailField label={t("Members")} value={group.member_count} />
                  <DetailField
                    label={t("Default API key policy")}
                    value={policyName}
                  />
                </dl>
              </CardContent>
            </Card>
          ) : null
        }
        editCard={
          <div className="flex flex-col gap-6">
            <Card>
              <CardHeader>
                <CardTitle>
                  {isNew ? t("Create user group") : t("Edit user group")}
                </CardTitle>
                <CardDescription>
                  {t("Users inherit this policy unless they have an individual override.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <div className="flex flex-col gap-4">
                  <FieldGroup>
                    <Field data-invalid={Boolean(fieldError("name"))}>
                      <FieldLabel htmlFor="user_group_name">
                        {t("Name")}
                      </FieldLabel>
                      <Input
                        id="user_group_name"
                        value={state.name}
                        onChange={(event) => patch({ name: event.target.value })}
                        aria-invalid={Boolean(fieldError("name"))}
                      />
                      {fieldError("name") ? (
                        <FieldError>{fieldError("name")}</FieldError>
                      ) : null}
                    </Field>
                    <Field data-invalid={Boolean(fieldError("description"))}>
                      <FieldLabel htmlFor="user_group_description">
                        {t("Description")}
                      </FieldLabel>
                      <Textarea
                        id="user_group_description"
                        value={state.description}
                        onChange={(event) =>
                          patch({ description: event.target.value })
                        }
                        aria-invalid={Boolean(fieldError("description"))}
                      />
                      <FieldDescription>
                        {t("Optional note shown to administrators.")}
                      </FieldDescription>
                      {fieldError("description") ? (
                        <FieldError>{fieldError("description")}</FieldError>
                      ) : null}
                    </Field>
                    <Field>
                      <FieldLabel htmlFor="user_group_policy">
                        {t("Default API key policy")}
                      </FieldLabel>
                      <Select
                        value={state.default_api_key_policy_id || "__none__"}
                        onValueChange={(value) =>
                          patch({
                            default_api_key_policy_id:
                              value === "__none__" ? "" : value,
                          })
                        }
                      >
                        <SelectTrigger id="user_group_policy">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem value="__none__">{t("None")}</SelectItem>
                            {policies.data
                              ?.filter(
                                (policy) =>
                                  policy.enabled ||
                                  policy.id === state.default_api_key_policy_id,
                              )
                              .map((policy) => (
                                <SelectItem key={policy.id} value={policy.id}>
                                  {policy.name}
                                </SelectItem>
                              ))}
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                      <FieldDescription>
                        {t("Users without an override inherit this policy immediately.")}
                      </FieldDescription>
                    </Field>
                  </FieldGroup>
                  <Button
                    className="self-start"
                    onClick={() => void submit()}
                    disabled={pending}
                  >
                    {create.isPending || update.isPending ? (
                      <Spinner data-icon="inline-start" />
                    ) : null}
                    {isNew ? t("Create user group") : t("Save user group")}
                  </Button>
                </div>
              </CardContent>
            </Card>

            {!isNew && group ? (
              <Card>
                <CardHeader>
                  <CardTitle>{t("Danger zone")}</CardTitle>
                  <CardDescription>
                    {t("Deleting a custom group is permanent and audited.")}
                  </CardDescription>
                </CardHeader>
                <CardContent className="flex flex-col items-start gap-4">
                  {group.system_role ? (
                    <Alert>
                      <AlertTitle>{t("Protected default group")}</AlertTitle>
                      <AlertDescription>
                        {t("Built-in default groups cannot be deleted.")}
                      </AlertDescription>
                    </Alert>
                  ) : (
                    <>
                      {group.member_count > 0 ? (
                        <Alert>
                          <AlertTitle>{t("Group still has members")}</AlertTitle>
                          <AlertDescription>
                            {t("Move every member out of this group before deleting it.")}
                          </AlertDescription>
                        </Alert>
                      ) : null}
                      <Button
                        variant="destructive"
                        disabled={pending || group.member_count > 0}
                        onClick={() => setDeleteOpen(true)}
                      >
                        {remove.isPending ? (
                          <Spinner data-icon="inline-start" />
                        ) : null}
                        {t("Delete user group")}
                      </Button>
                    </>
                  )}
                </CardContent>
              </Card>
            ) : null}
          </div>
        }
      />
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={t("Delete user group?")}
        description={t(
          "This permanently deletes the empty group. This action cannot be undone.",
        )}
        confirmLabel={t("Delete user group")}
        destructive
        onConfirm={() => void deleteGroup()}
      />
    </>
  );
}
