import { useEffect } from "react";
import { useParams } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useUpstreamAccesses, useLogicalChannels, useUsers } from "@/features/admin/api";
import { useSaveSharing, useSharingGroup, useSharingSeats } from "./api";
import { SharingUsage } from "./usage";
import { useReturnPath, withReturnTo } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

const money = z.string().regex(/^\d+(\.\d{1,8})?$/, "Enter a positive USD amount with up to 8 decimal places.")
  .refine(value => Number(value) > 0 && Number(value) <= 1_000_000, "Amount must be positive and at most 1000000.");
const limit = z.number().int().min(1).max(100_000);
const schema = z.object({
  name: z.string().trim().min(1).max(120),
  channel_id: z.guid(),
  enabled: z.boolean(),
  seats: z.array(z.guid().nullable()).min(1).max(100)
    .refine(seats => new Set(seats.filter(Boolean)).size === seats.filter(Boolean).length, "A user can occupy only one seat."),
  primary_limit_amount: money, secondary_limit_amount: money, request_reservation_amount: money,
  user_requests_per_minute: limit, group_requests_per_minute: limit,
  user_max_concurrent_requests: limit, group_max_concurrent_requests: limit,
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = {
  name: "", channel_id: "", enabled: false, seats: [null],
  primary_limit_amount: "20", secondary_limit_amount: "100", request_reservation_amount: "0.10",
  user_requests_per_minute: 30, group_requests_per_minute: 120,
  user_max_concurrent_requests: 1, group_max_concurrent_requests: 4,
};
const amountFields = [
  ["primary_limit_amount", "Primary window total (USD)"],
  ["secondary_limit_amount", "Secondary window total (USD)"],
  ["request_reservation_amount", "Reserve per request (USD)"],
] as const;
const rateFields = [
  ["user_requests_per_minute", "User RPM"], ["group_requests_per_minute", "Car RPM"],
  ["user_max_concurrent_requests", "User concurrency"], ["group_max_concurrent_requests", "Car concurrency"],
] as const;

export function SharingDetailPage() {
  const { id = "new" } = useParams();
  const isNew = id === "new";
  const detail = useSharingGroup(id);
  const usage = useSharingSeats(id);
  const save = useSaveSharing(id);
  const users = useUsers();
  const accesses = useUpstreamAccesses();
  const channels = useLogicalChannels();
  const returnTo = useReturnPath("/admin/codex-sharing");
  const { t } = useI18n();
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(save.isPending, form.formState.isDirty);
  useEffect(() => {
    form.reset(detail.data?.data ?? defaults);
  }, [detail.data, id, form]);
  const values = form.watch();
  const errors = form.formState.errors;
  const eligibleChannels = (channels.data ?? []).filter(channel =>
    channel.credential_id && accesses.data?.some(access =>
      access.id === channel.access_id && access.connector_kind === "codex"));
  const members = users.data ?? [];
  const formerMembers = values.seats.filter((userId): userId is string =>
    userId !== null && !users.data?.some(user => user.id === userId));
  const submit = form.handleSubmit(async input => {
    if (Number(input.request_reservation_amount) > Math.min(Number(input.primary_limit_amount), Number(input.secondary_limit_amount)) / input.seats.length) {
      form.setError("request_reservation_amount", { message: t("Reservation must fit each seat allowance.") });
      return;
    }
    try {
      const result = await save.mutateAsync({ input, etag: detail.data?.etag });
      form.reset(input);
      markSaved();
      toast.success(t("Sharing group saved"));
      if (isNew) navigate(withReturnTo(`/admin/codex-sharing/${result.id}`, returnTo), { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.isConflict && !isNew) {
        toast.error(t("This sharing group changed elsewhere. Reloading."));
        await detail.refetch();
      } else {
        toast.error(error instanceof Error ? error.message : t("Save failed"));
      }
    }
  });
  return <div className="flex min-w-0 flex-col gap-6">
    {navigationGuard}
    <PageHeader title={isNew ? "New sharing group" : detail.data?.data.name ?? "Codex sharing"}
      backTo={returnTo} backLabel={t("Back to sharing groups")} />
    <AsyncResource isLoading={detail.isLoading || users.isLoading || channels.isLoading || accesses.isLoading}
      error={detail.error ?? users.error ?? channels.error ?? accesses.error}>
      <Alert><AlertDescription>
        {t("Soft USD allowance, not official credits. Active requests can exceed their reservation. No automatic fallback, borrowing or rollover.")}
      </AlertDescription></Alert>
      {isNew && <Alert variant="destructive"><AlertDescription>
        {t("A car binds one logical channel and its upstream account, even while paused. The same account cannot join another car or bypass protection through an alias. Unrelated ordinary channels remain usable.")}
      </AlertDescription></Alert>}
      <form onSubmit={submit} className="flex flex-col gap-6">
        <Card>
          <CardHeader><CardTitle>{t("Sharing configuration")}</CardTitle>
            <CardDescription>{t("The channel and account binding cannot change after creation. Seats are independent of user groups.")}</CardDescription></CardHeader>
          <CardContent><FieldGroup className="grid gap-6 md:grid-cols-2">
            <Field className="md:col-span-2" data-invalid={!!errors.name}><FieldLabel htmlFor="sharing-name">{t("Name")}</FieldLabel>
              <Input id="sharing-name" {...form.register("name")} aria-invalid={!!errors.name} />
              <FieldError errors={[errors.name]} /></Field>
            <Field className="md:col-span-2" data-invalid={!!errors.channel_id}>
              <FieldLabel htmlFor="sharing-channel">{t("Logical channel")}</FieldLabel>
              <Select disabled={!isNew} value={values.channel_id}
                items={eligibleChannels.map(channel => ({ value: channel.id, label: channel.name }))}
                onValueChange={value => form.setValue("channel_id", value ?? "", { shouldDirty: true })}>
                <SelectTrigger id="sharing-channel" aria-invalid={!!errors.channel_id}><SelectValue placeholder={t("Select channel")} /></SelectTrigger>
                <SelectContent><SelectGroup>{eligibleChannels.map(channel =>
                  <SelectItem key={channel.id} value={channel.id}>{channel.name}</SelectItem>)}</SelectGroup></SelectContent>
              </Select><FieldError errors={[errors.channel_id]} />
            </Field>
            <Field orientation="horizontal" className="md:col-span-2"><FieldLabel htmlFor="sharing-enabled">{t("Enabled")}</FieldLabel>
              <Switch id="sharing-enabled" checked={values.enabled}
                onCheckedChange={value => form.setValue("enabled", value, { shouldDirty: true })} /></Field>
          </FieldGroup><FieldGroup className="mt-6 grid gap-6 md:grid-cols-3">
            {amountFields.map(([name, label]) => <Field key={name} data-invalid={!!errors[name]}>
              <FieldLabel htmlFor={`sharing-${name}`}>{t(label)}</FieldLabel>
              <Input id={`sharing-${name}`} inputMode="decimal" {...form.register(name)} aria-invalid={!!errors[name]} />
              <FieldError errors={[errors[name]]} />
            </Field>)}
          </FieldGroup><FieldGroup className="mt-6 grid gap-6 sm:grid-cols-2 lg:grid-cols-4">
            {rateFields.map(([name, label]) => <Field key={name} data-invalid={!!errors[name]}>
              <FieldLabel htmlFor={`sharing-${name}`}>{t(label)}</FieldLabel>
              <Input id={`sharing-${name}`} type="number" min={1} max={100000}
                {...form.register(name, { valueAsNumber: true })} aria-invalid={!!errors[name]} />
              <FieldError errors={[errors[name]]} />
            </Field>)}
          </FieldGroup></CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle>{t("Fixed seats")}</CardTitle>
            <CardDescription>{t("Vacant seats retain their share. Replacements inherit remaining allowance; expansion and budget changes wait for the next window.")}</CardDescription></CardHeader>
          <CardContent><FieldGroup className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
            {values.seats.map((userId, index) => <Field key={index}>
              <FieldLabel htmlFor={`sharing-seat-${index}`}>{t("Seat")} {index + 1}</FieldLabel>
              <Select value={userId ?? "vacant"}
                items={[{ value: "vacant", label: t("Vacant") },
                  ...formerMembers.map(id => ({ value: id, label: t("Former member") })),
                  ...members.map(user => ({ value: user.id, label: user.display_name || user.email }))]}
                onValueChange={value => form.setValue(`seats.${index}`, !value || value === "vacant" ? null : value, { shouldDirty: true })}>
                <SelectTrigger id={`sharing-seat-${index}`}><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>
                  <SelectItem value="vacant">{t("Vacant")}</SelectItem>
                  {formerMembers.map(id => <SelectItem key={id} value={id} disabled>{t("Former member")}</SelectItem>)}
                  {members.map(user => <SelectItem key={user.id} value={user.id}
                    disabled={user.status !== "active"
                      || values.seats.some((selected, slot) => selected === user.id && slot !== index)}>
                    {user.display_name || user.email}
                  </SelectItem>)}
                </SelectGroup></SelectContent>
              </Select>
            </Field>)}
            <FieldError className="col-span-full" errors={[errors.seats]} />
            <FieldDescription className="col-span-full">{t("All keys of a user share one allowance. Moving between seats does not clear personal window spend.")}</FieldDescription>
            <div className="col-span-full flex flex-wrap gap-2">
              <Button type="button" variant="outline" disabled={values.seats.length >= 100}
                onClick={() => form.setValue("seats", [...values.seats, null], { shouldDirty: true })}>{t("Add seat")}</Button>
              <Button type="button" variant="outline" disabled={values.seats.length <= (detail.data?.data.seats.length ?? 1)}
                onClick={() => form.setValue("seats", values.seats.slice(0, -1), { shouldDirty: true })}>{t("Remove last seat")}</Button>
            </div>
          </FieldGroup></CardContent>
        </Card>
        <Button type="submit" className="w-full sm:w-auto sm:self-start" disabled={save.isPending}>{t(save.isPending ? "Saving..." : "Save")}</Button>
      </form>
      {!isNew && <AsyncResource isLoading={usage.isLoading} error={usage.error}>
        {usage.data?.seats.map(seat => <Card key={seat.seat_number}>
          <CardHeader><CardTitle>{t("Seat")} {seat.seat_number}</CardTitle>
            <CardDescription>{users.data?.find(user => user.id === seat.user_id)?.display_name
              ?? t(seat.user_id ? "Former member" : "Vacant")}</CardDescription></CardHeader>
          <CardContent><SharingUsage usage={seat.usage} /></CardContent>
        </Card>)}
      </AsyncResource>}
    </AsyncResource>
  </div>;
}
