import { useEffect, useState } from "react";
import { z } from "zod";
import { toast } from "sonner";
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
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ChannelBatchChanges, ChannelView } from "@/api/types";
import { useBatchUpdateChannels } from "@/features/admin/api";
import { useI18n } from "@/app/i18n";

type BooleanChange = "unchanged" | "true" | "false";

const MAX_BATCH_SIZE = 100;
const decimalPattern = /^(?:0|[1-9]\d*)(?:\.\d+)?$/;
const schema = z
  .object({
    enabled: z.enum(["unchanged", "true", "false"]),
    status_statistics_enabled: z.enum(["unchanged", "true", "false"]),
    auto_disable_allowed: z.enum(["unchanged", "true", "false"]),
    weight: z.string().trim(),
    billing_multiplier: z.string().trim(),
  })
  .superRefine((value, context) => {
    if (value.weight !== "") {
      const weight = Number(value.weight);
      if (!Number.isInteger(weight) || weight < 1) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["weight"],
          message: "Weight must be at least 1.",
        });
      }
    }
    if (
      value.billing_multiplier !== "" &&
      (!decimalPattern.test(value.billing_multiplier) ||
        !Number.isFinite(Number(value.billing_multiplier)))
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["billing_multiplier"],
        message: "Billing multiplier must be zero or greater.",
      });
    }
    if (
      value.enabled === "unchanged" &&
      value.status_statistics_enabled === "unchanged" &&
      value.auto_disable_allowed === "unchanged" &&
      value.weight === "" &&
      value.billing_multiplier === ""
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
  enabled: "unchanged",
  status_statistics_enabled: "unchanged",
  auto_disable_allowed: "unchanged",
  weight: "",
  billing_multiplier: "",
};

function booleanChange(value: BooleanChange): boolean | undefined {
  if (value === "unchanged") return undefined;
  return value === "true";
}

interface ChannelBatchEditDialogProps {
  open: boolean;
  channels: ChannelView[];
  onOpenChange: (open: boolean) => void;
  onApplied: () => void;
}

export function ChannelBatchEditDialog({
  open,
  channels,
  onOpenChange,
  onApplied,
}: ChannelBatchEditDialogProps) {
  const update = useBatchUpdateChannels();
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const exceedsBatchLimit = channels.length > MAX_BATCH_SIZE;

  useEffect(() => {
    if (!open) {
      setState(empty);
      setValidation(null);
    }
  }, [open]);

  const patch = (partial: Partial<FormState>) =>
    setState((current) => ({ ...current, ...partial }));
  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)?.message;
    return message ? t(message) : undefined;
  };

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    const changes: ChannelBatchChanges = {};
    const enabled = booleanChange(parsed.data.enabled);
    const statusStatisticsEnabled = booleanChange(
      parsed.data.status_statistics_enabled,
    );
    const autoDisableAllowed = booleanChange(parsed.data.auto_disable_allowed);
    if (enabled !== undefined) changes.enabled = enabled;
    if (statusStatisticsEnabled !== undefined) {
      changes.status_statistics_enabled = statusStatisticsEnabled;
    }
    if (autoDisableAllowed !== undefined) {
      changes.auto_disable_allowed = autoDisableAllowed;
    }
    if (parsed.data.weight !== "") {
      changes.weight = Number(parsed.data.weight);
    }
    if (parsed.data.billing_multiplier !== "") {
      changes.billing_multiplier = parsed.data.billing_multiplier;
    }

    setValidation(null);
    try {
      const result = await update.mutateAsync({
        items: channels.map((channel) => ({
          id: channel.id,
          updated_at: channel.updated_at,
        })),
        changes,
      });
      toast.success(
        t("Updated {count} channels.", { count: result.updated_ids.length }),
      );
      onApplied();
      onOpenChange(false);
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("One or more channels changed elsewhere. Refresh and try again."));
      } else {
        toast.error(
          controlPlaneMutationErrorMessage(error, t("Batch update failed")),
        );
      }
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!update.isPending) onOpenChange(nextOpen);
      }}
    >
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("Batch edit channels")}</DialogTitle>
          <DialogDescription>
            {t("Apply the selected changes atomically to {count} channels.", {
              count: channels.length,
            })}
          </DialogDescription>
        </DialogHeader>

        <FieldGroup>
          <Field>
            <FieldLabel>{t("Enabled state")}</FieldLabel>
            <Select
              value={state.enabled}
              onValueChange={(value) => patch({ enabled: value as BooleanChange })}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="unchanged">{t("Keep unchanged")}</SelectItem>
                  <SelectItem value="true">{t("Enabled")}</SelectItem>
                  <SelectItem value="false">{t("Disabled")}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </Field>
          <Field>
            <FieldLabel>{t("Status statistics")}</FieldLabel>
            <Select
              value={state.status_statistics_enabled}
              onValueChange={(value) =>
                patch({ status_statistics_enabled: value as BooleanChange })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="unchanged">{t("Keep unchanged")}</SelectItem>
                  <SelectItem value="true">{t("Enabled")}</SelectItem>
                  <SelectItem value="false">{t("Disabled")}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </Field>
          <Field>
            <FieldLabel>{t("Allow automatic disable")}</FieldLabel>
            <Select
              value={state.auto_disable_allowed}
              onValueChange={(value) =>
                patch({ auto_disable_allowed: value as BooleanChange })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="unchanged">{t("Keep unchanged")}</SelectItem>
                  <SelectItem value="true">{t("Allowed")}</SelectItem>
                  <SelectItem value="false">{t("Not allowed")}</SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </Field>
          <Field data-invalid={Boolean(fieldError("weight"))}>
            <FieldLabel htmlFor="batch-channel-weight">{t("Weight")}</FieldLabel>
            <Input
              id="batch-channel-weight"
              type="number"
              min={1}
              value={state.weight}
              placeholder={t("Keep unchanged")}
              onChange={(event) => patch({ weight: event.target.value })}
              aria-invalid={Boolean(fieldError("weight"))}
            />
            {fieldError("weight") ? <FieldError>{fieldError("weight")}</FieldError> : null}
          </Field>
          <Field data-invalid={Boolean(fieldError("billing_multiplier"))}>
            <FieldLabel htmlFor="batch-channel-billing-multiplier">
              {t("Billing multiplier")}
            </FieldLabel>
            <Input
              id="batch-channel-billing-multiplier"
              inputMode="decimal"
              value={state.billing_multiplier}
              placeholder={t("Keep unchanged")}
              onChange={(event) =>
                patch({ billing_multiplier: event.target.value })
              }
              aria-invalid={Boolean(fieldError("billing_multiplier"))}
            />
            <FieldDescription>
              {t("Multiplies the upstream model price used for request settlement.")}
            </FieldDescription>
            {fieldError("billing_multiplier") ? (
              <FieldError>{fieldError("billing_multiplier")}</FieldError>
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
                {t("Select at most {count} channels per batch.", {
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
            onClick={submit}
            disabled={
              update.isPending || channels.length === 0 || exceedsBatchLimit
            }
          >
            {update.isPending ? <Spinner data-icon="inline-start" /> : null}
            {t("Update channels")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
