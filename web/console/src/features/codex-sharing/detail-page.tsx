import { useEffect } from "react";
import { Link, useNavigate, useParams } from "react-router";
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
import { useChannelGroups, useChannels, useUserGroups, useUsers } from "@/features/admin/api";
import { useSaveSharing, useSharingGroup, useSharingSeats } from "./api";
import { SharingUsage } from "./usage";

const money = z.string().regex(/^\d+(\.\d{1,8})?$/, "Enter a positive USD amount with up to 8 decimal places.")
  .refine(value => Number(value) > 0 && Number(value) <= 1_000_000, "Amount must be positive and at most 1000000.");
const limit = z.number().int().min(1).max(100_000);
const schema = z.object({
  name: z.string().trim().min(1).max(120),
  user_group_id: z.guid(),
  credential_id: z.guid(),
  enabled: z.boolean(),
  seats: z.array(z.guid().nullable()).min(1).max(100)
    .refine(seats => new Set(seats.filter(Boolean)).size === seats.filter(Boolean).length, "A user can occupy only one seat."),
  primary_limit_amount: money, secondary_limit_amount: money, request_reservation_amount: money,
  user_requests_per_minute: limit, group_requests_per_minute: limit,
  user_max_concurrent_requests: limit, group_max_concurrent_requests: limit,
});
type FormValues = z.infer<typeof schema>;
const defaults: FormValues = {
  name: "", user_group_id: "", credential_id: "", enabled: false, seats: [null],
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
  const userGroups = useUserGroups();
  const users = useUsers();
  const channelGroups = useChannelGroups();
  const channels = useChannels();
  const navigate = useNavigate();
  const { t } = useI18n();
  const form = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: defaults });
  useEffect(() => {
    form.reset(detail.data?.data ?? defaults);
  }, [detail.data, id, form]);
  const values = form.watch();
  const errors = form.formState.errors;
  const credentials = (channels.data ?? []).filter(channel =>
    channel.api_format === "open_ai_responses" && channelGroups.data?.some(group =>
      group.id === channel.channel_group_id && group.connector_kind === "codex_oauth"));
  const members = (users.data ?? []).filter(user =>
    user.user_group_id === values.user_group_id || values.seats.includes(user.id));
  const formerMembers = values.seats.filter((userId): userId is string =>
    userId !== null && !users.data?.some(user => user.id === userId));
  const submit = form.handleSubmit(async input => {
    if (Number(input.request_reservation_amount) > Math.min(Number(input.primary_limit_amount), Number(input.secondary_limit_amount)) / input.seats.length) {
      form.setError("request_reservation_amount", { message: t("Reservation must fit each seat allowance.") });
      return;
    }
    try {
      const result = await save.mutateAsync({ input, etag: detail.data?.etag });
      toast.success(t("Sharing group saved"));
      if (isNew) navigate(`/admin/codex-sharing/${result.id}`, { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.isConflict && !isNew) {
        toast.error(t("This sharing group changed elsewhere. Reloading."));
        await detail.refetch();
      } else {
        toast.error(error instanceof Error ? error.message : t("Save failed"));
      }
    }
  });
  return <div className="flex flex-col gap-6">
    <PageHeader title={isNew ? "New sharing group" : detail.data?.data.name ?? "Codex sharing"}
      actions={<Button variant="outline" nativeButton={false} render={<Link to="/admin/codex-sharing" />}>{t("Back")}</Button>} />
    <AsyncResource isLoading={detail.isLoading || users.isLoading || userGroups.isLoading || channels.isLoading || channelGroups.isLoading}
      error={detail.error ?? users.error ?? userGroups.error ?? channels.error ?? channelGroups.error}>
      <Alert><AlertDescription>
        {t("Soft USD allowance, not official credits. Active requests can exceed their reservation. No automatic fallback, borrowing or rollover.")}
      </AlertDescription></Alert>
      {isNew && <Alert variant="destructive"><AlertDescription>
        {t("Creating a sharing group immediately restricts all keys in the selected user group, even while paused. Bindings cannot be undone in this version. Use a dedicated user group.")}
      </AlertDescription></Alert>}
      <form onSubmit={submit} className="flex flex-col gap-6">
        <Card>
          <CardHeader><CardTitle>{t("Sharing configuration")}</CardTitle>
            <CardDescription>{t("Credential and user group cannot be changed after creation.")}</CardDescription></CardHeader>
          <CardContent><FieldGroup className="grid gap-6 md:grid-cols-2">
            <Field className="md:col-span-2" data-invalid={!!errors.name}><FieldLabel htmlFor="sharing-name">{t("Name")}</FieldLabel>
              <Input id="sharing-name" {...form.register("name")} aria-invalid={!!errors.name} />
              <FieldError errors={[errors.name]} /></Field>
            <Field data-invalid={!!errors.user_group_id}>
              <FieldLabel htmlFor="sharing-user-group">{t("User group")}</FieldLabel>
              <Select disabled={!isNew} value={values.user_group_id}
                items={userGroups.data?.map(group => ({ value: group.id, label: group.name }))}
                onValueChange={value => {
                  form.setValue("user_group_id", value ?? "", { shouldDirty: true });
                  form.setValue("seats", values.seats.map(() => null), { shouldDirty: true });
                }}>
                <SelectTrigger id="sharing-user-group" aria-invalid={!!errors.user_group_id}><SelectValue placeholder={t("Select user group")} /></SelectTrigger>
                <SelectContent><SelectGroup>{userGroups.data?.map(group =>
                  <SelectItem key={group.id} value={group.id}>{group.name}</SelectItem>)}</SelectGroup></SelectContent>
              </Select><FieldError errors={[errors.user_group_id]} />
            </Field>
            <Field data-invalid={!!errors.credential_id}>
              <FieldLabel htmlFor="sharing-credential">{t("Codex credential")}</FieldLabel>
              <Select disabled={!isNew} value={values.credential_id}
                items={credentials.map(credential => ({ value: credential.id, label: credential.name }))}
                onValueChange={value => form.setValue("credential_id", value ?? "", { shouldDirty: true })}>
                <SelectTrigger id="sharing-credential" aria-invalid={!!errors.credential_id}><SelectValue placeholder={t("Select credential")} /></SelectTrigger>
                <SelectContent><SelectGroup>{credentials.map(credential =>
                  <SelectItem key={credential.id} value={credential.id}>{credential.name}</SelectItem>)}</SelectGroup></SelectContent>
              </Select><FieldError errors={[errors.credential_id]} />
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
                    disabled={user.status !== "active" || user.user_group_id !== values.user_group_id
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
