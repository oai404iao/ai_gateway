import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ChannelCapabilityView } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useBatchUpdateCapabilities } from "@/features/admin/api";

const unchanged = "unchanged";
const choice = z.enum([unchanged, "true", "false"]);
const schema = z.object({
  enabled: choice,
  auto_disable_allowed: choice,
  billing_multiplier: z.string().trim().refine((value) => value === "" || /^\d+(?:\.\d+)?$/.test(value), "Enter a non-negative decimal."),
}).refine((value) => value.enabled !== unchanged || value.auto_disable_allowed !== unchanged || value.billing_multiplier !== "", {
  message: "Choose at least one field to change.",
  path: ["enabled"],
});
type Values = z.infer<typeof schema>;

export function CapabilityBatchDialog({ capabilities, onClose, onApplied }: {
  capabilities: ChannelCapabilityView[];
  onClose: () => void;
  onApplied: () => void;
}) {
  const { t } = useI18n();
  const mutation = useBatchUpdateCapabilities();
  const form = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { enabled: unchanged, auto_disable_allowed: unchanged, billing_multiplier: "" },
  });
  const submit = form.handleSubmit(async (values) => {
    try {
      await mutation.mutateAsync({
        items: capabilities.map(({ id, updated_at }) => ({ id, updated_at })),
        changes: {
          ...(values.enabled === unchanged ? {} : { enabled: values.enabled === "true" }),
          ...(values.auto_disable_allowed === unchanged ? {} : { auto_disable_allowed: values.auto_disable_allowed === "true" }),
          ...(values.billing_multiplier === "" ? {} : { billing_multiplier: values.billing_multiplier }),
        },
      });
      toast.success(t("Capabilities updated"));
      onApplied();
      onClose();
    } catch (error) {
      toast.error(t(error instanceof ApiError && error.isConflict
        ? "One or more capabilities changed elsewhere. Refresh and try again."
        : controlPlaneMutationErrorMessage(error, "Could not update capabilities.")));
    }
  });
  return (
    <Dialog open onOpenChange={(open) => { if (!open && !mutation.isPending) onClose(); }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("Batch edit capabilities")}</DialogTitle>
          <DialogDescription>{t("Update {count} capabilities atomically. Unchanged fields and API key grants are preserved.", { count: capabilities.length })}</DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-4">
          <FieldGroup>
            {(["enabled", "auto_disable_allowed"] as const).map((name) => (
              <Field key={name} data-invalid={Boolean(form.formState.errors[name])}>
                <FieldLabel htmlFor={`batch-${name}`}>{t(name === "enabled" ? "Enabled" : "Allow automatic disable")}</FieldLabel>
                <Select value={form.watch(name)} onValueChange={(value) => form.setValue(name, value as Values[typeof name], { shouldDirty: true })}>
                  <SelectTrigger id={`batch-${name}`} aria-invalid={Boolean(form.formState.errors[name])}><SelectValue /></SelectTrigger>
                  <SelectContent><SelectGroup>
                    <SelectItem value={unchanged}>{t("Unchanged")}</SelectItem>
                    <SelectItem value="true">{t("Yes")}</SelectItem>
                    <SelectItem value="false">{t("No")}</SelectItem>
                  </SelectGroup></SelectContent>
                </Select>
                <FieldError>{form.formState.errors[name]?.message && t(form.formState.errors[name].message)}</FieldError>
              </Field>
            ))}
            <Field data-invalid={Boolean(form.formState.errors.billing_multiplier)}>
              <FieldLabel htmlFor="batch-multiplier">{t("Billing multiplier")}</FieldLabel>
              <Input id="batch-multiplier" placeholder={t("Unchanged")} aria-invalid={Boolean(form.formState.errors.billing_multiplier)} {...form.register("billing_multiplier")} />
              <FieldError>{form.formState.errors.billing_multiplier?.message && t(form.formState.errors.billing_multiplier.message)}</FieldError>
            </Field>
          </FieldGroup>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose} disabled={mutation.isPending}>{t("Cancel")}</Button>
            <Button type="submit" disabled={mutation.isPending || capabilities.length === 0 || capabilities.length > 100}>{t("Apply changes")}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
