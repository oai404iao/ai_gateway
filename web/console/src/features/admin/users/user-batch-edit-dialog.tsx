import { useEffect, useState } from "react";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import type {
  ApiKeyPolicyView,
  ControlPlaneUser,
  UserBatchChanges,
  UserGroupView,
} from "@/api/types";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import { useBatchUpdateUsers } from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { userStatusLabel } from "@/lib/permissions";

const MAX_BATCH_SIZE = 100;
const decimalPattern = /^(?:0|[1-9]\d*)(?:\.\d+)?$/;
const schema = z
  .object({
    status: z.enum(["unchanged", "active", "suspended", "disabled"]),
    user_group_id: z.string(),
    policy: z.string(),
    balance_operation: z.enum([
      "unchanged",
      "set",
      "increase",
      "decrease",
    ]),
    balance_amount: z.string().trim(),
  })
  .superRefine((value, context) => {
    if (
      value.balance_operation !== "unchanged" &&
      (!decimalPattern.test(value.balance_amount) ||
        !Number.isFinite(Number(value.balance_amount)))
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["balance_amount"],
        message: "Enter a valid non-negative balance.",
      });
    }
    if (
      value.status === "unchanged" &&
      value.user_group_id === "__unchanged__" &&
      value.policy === "__unchanged__" &&
      value.balance_operation === "unchanged"
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["changes"],
        message: "Choose at least one field to change.",
      });
    }
  });

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  status: "unchanged",
  user_group_id: "__unchanged__",
  policy: "__unchanged__",
  balance_operation: "unchanged",
  balance_amount: "",
};

interface UserBatchEditDialogProps {
  open: boolean;
  users: ControlPlaneUser[];
  groups: UserGroupView[];
  policies: ApiKeyPolicyView[];
  currentUserId?: string;
  onOpenChange: (open: boolean) => void;
  onApplied: () => void;
  triggerId?: string;
}

export function UserBatchEditDialog({
  open,
  users,
  groups,
  policies,
  currentUserId,
  onOpenChange,
  onApplied,
  triggerId,
}: UserBatchEditDialogProps) {
  const update = useBatchUpdateUsers();
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const exceedsBatchLimit = users.length > MAX_BATCH_SIZE;
  const includesInvited = users.some((user) => user.status === "invited");
  const includesCurrentUser = users.some((user) => user.id === currentUserId);

  useEffect(() => {
    if (!open) {
      setState(empty);
      setValidation(null);
    }
  }, [open]);

  const patch = (partial: Partial<FormState>) =>
    setState((current) => ({ ...current, ...partial }));
  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)
      ?.message;
    return message ? t(message) : undefined;
  };

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    if (parsed.data.status !== "unchanged" && includesInvited) {
      setValidation(
        new z.ZodError([
          {
            code: z.ZodIssueCode.custom,
            path: ["status"],
            message:
              "Pending invitations cannot be batch-updated to a runtime status.",
          },
        ]),
      );
      return;
    }
    if (
      includesCurrentUser &&
      matchesDisabledStatus(parsed.data.status)
    ) {
      setValidation(
        new z.ZodError([
          {
            code: z.ZodIssueCode.custom,
            path: ["status"],
            message: "You cannot suspend or disable your own account in a batch.",
          },
        ]),
      );
      return;
    }

    const changes: UserBatchChanges = {};
    if (parsed.data.status !== "unchanged") {
      changes.status = parsed.data.status;
    }
    if (parsed.data.user_group_id !== "__unchanged__") {
      changes.user_group_id = parsed.data.user_group_id;
    }
    if (parsed.data.policy === "__inherit__") {
      changes.default_api_key_policy_id = null;
    } else if (parsed.data.policy !== "__unchanged__") {
      changes.default_api_key_policy_id = parsed.data.policy;
    }
    if (parsed.data.balance_operation !== "unchanged") {
      changes.balance = {
        operation: parsed.data.balance_operation,
        amount: parsed.data.balance_amount,
      };
    }

    setValidation(null);
    try {
      const result = await update.mutateAsync({
        items: users.map((user) => ({
          id: user.id,
          updated_at: user.updated_at,
        })),
        changes,
      });
      toast.success(
        t("Updated {count} users.", { count: result.updated_ids.length }),
      );
      onApplied();
      onOpenChange(false);
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("One or more users changed elsewhere. Refresh and try again."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Batch update failed"));
      }
    }
  };

  return (
    <Dialog
      open={open}
      triggerId={triggerId}
      onOpenChange={(nextOpen) => {
        if (!update.isPending) onOpenChange(nextOpen);
      }}
    >
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("Batch edit users")}</DialogTitle>
          <DialogDescription>
            {t("Apply the selected changes atomically to {count} users.", {
              count: users.length,
            })}
          </DialogDescription>
        </DialogHeader>

        <FieldGroup>
          <Field data-invalid={Boolean(fieldError("status"))}>
            <FieldLabel>{t("Status")}</FieldLabel>
            <Select
              value={state.status}
              onValueChange={(value) =>
                patch({ status: value as FormState["status"] })
              }
            >
              <SelectTrigger aria-invalid={Boolean(fieldError("status"))}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="unchanged">{t("Keep unchanged")}</SelectItem>
                  {(["active", "suspended", "disabled"] as const).map((status) => (
                    <SelectItem key={status} value={status}>
                      {userStatusLabel(status)}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
            {includesInvited ? (
              <FieldDescription>
                {t("Leave status unchanged when pending invitations are selected.")}
              </FieldDescription>
            ) : null}
            {fieldError("status") ? (
              <FieldError>{fieldError("status")}</FieldError>
            ) : null}
          </Field>

          <Field>
            <FieldLabel>{t("User group")}</FieldLabel>
            <Select
              value={state.user_group_id}
              onValueChange={(value) => patch({ user_group_id: value })}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="__unchanged__">
                    {t("Keep unchanged")}
                  </SelectItem>
                  {groups.map((group) => (
                    <SelectItem key={group.id} value={group.id}>
                      {group.name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </Field>

          <Field>
            <FieldLabel>{t("API policy override")}</FieldLabel>
            <Select
              value={state.policy}
              onValueChange={(value) => patch({ policy: value })}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="__unchanged__">
                    {t("Keep unchanged")}
                  </SelectItem>
                  <SelectItem value="__inherit__">
                    {t("Inherit group policy")}
                  </SelectItem>
                  {policies
                    .filter((policy) => policy.enabled)
                    .map((policy) => (
                      <SelectItem key={policy.id} value={policy.id}>
                        {policy.name}
                      </SelectItem>
                    ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </Field>

          <Field data-invalid={Boolean(fieldError("balance_amount"))}>
            <FieldLabel>{t("Balance change")}</FieldLabel>
            <Select
              value={state.balance_operation}
              onValueChange={(value) =>
                patch({
                  balance_operation: value as FormState["balance_operation"],
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="unchanged">{t("Keep unchanged")}</SelectItem>
                  <SelectItem value="set">{t("Set balance")}</SelectItem>
                  <SelectItem value="increase">{t("Increase balance")}</SelectItem>
                  <SelectItem value="decrease">{t("Decrease balance")}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
            {state.balance_operation !== "unchanged" ? (
              <Input
                aria-label={t("Balance amount")}
                inputMode="decimal"
                value={state.balance_amount}
                onChange={(event) => patch({ balance_amount: event.target.value })}
                aria-invalid={Boolean(fieldError("balance_amount"))}
                placeholder="0"
              />
            ) : null}
            <FieldDescription>
              {t("Increase and decrease apply the same USD amount to every selected user.")}
            </FieldDescription>
            {fieldError("balance_amount") ? (
              <FieldError>{fieldError("balance_amount")}</FieldError>
            ) : null}
          </Field>

          {fieldError("changes") ? (
            <Field data-invalid>
              <FieldError>{fieldError("changes")}</FieldError>
            </Field>
          ) : null}
          {exceedsBatchLimit ? (
            <Field data-invalid>
              <FieldError>
                {t("Select at most {count} users per batch.", {
                  count: MAX_BATCH_SIZE,
                })}
              </FieldError>
            </Field>
          ) : null}
        </FieldGroup>

        <DialogFooter>
          <DialogClose
            render={<Button variant="outline" disabled={update.isPending} />}
          >
            {t("Cancel")}
          </DialogClose>
          <Button
            onClick={() => void submit()}
            disabled={update.isPending || users.length === 0 || exceedsBatchLimit}
          >
            {update.isPending ? <Spinner data-icon="inline-start" /> : null}
            {t("Update users")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function matchesDisabledStatus(status: FormState["status"]): boolean {
  return status === "suspended" || status === "disabled";
}
