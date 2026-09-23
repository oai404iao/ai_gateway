import { useEffect, useMemo, useState } from "react";
import { useParams } from "react-router";
import { useReturnPath } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import {
  RoutingTargetFields,
  type RoutingTargetChannel,
  type RoutingTargetGroup,
} from "@/components/shared/routing-target-fields";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useApiKeyPolicy,
  useRoutingGroups,
  useLogicalChannels,
  useChannelCapabilities,
  useCreateApiKeyPolicy,
  useUpdateApiKeyPolicy,
} from "@/features/admin/api";
import { ApiError } from "@/api/errors";
import type { ApiKeyPolicyInput } from "@/api/types";
import { formatRelative } from "@/lib/dates";
import { useI18n } from "@/app/i18n";

const schema = z
  .object({
    name: z.string().min(1, "Name is required.").max(100),
    allowed_group_ids: z.array(z.string()),
    allowed_channel_ids: z.array(z.string()),
    enabled: z.boolean(),
  })
  .superRefine((value, context) => {
    if (value.allowed_group_ids.length === 0 && value.allowed_channel_ids.length === 0) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["allowed_group_ids"],
        message: "Pick at least one channel group or channel.",
      });
    }
  });

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  allowed_group_ids: [],
  allowed_channel_ids: [],
  enabled: true,
};

export function ApiKeyPolicyDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const returnTo = useReturnPath("/admin/api-key-policies");
  const { data, etag, isLoading, error } = useApiKeyPolicy(id);
  const groups = useRoutingGroups();
  const channels = useLogicalChannels();
  const capabilities = useChannelCapabilities();
  const create = useCreateApiKeyPolicy();
  const update = useUpdateApiKeyPolicy(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const draft = useConfigurationDraft(submitting);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        allowed_group_ids: data.data.allowed_group_ids,
        allowed_channel_ids: data.data.allowed_channel_ids,
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  const targetGroups = useMemo<RoutingTargetGroup[]>(
    () =>
      (groups.data ?? []).map((group) => ({
        id: group.id,
        name: group.name,
        enabled: group.enabled,
      })),
    [groups.data],
  );
  const targetChannels = useMemo<RoutingTargetChannel[]>(() => {
    const groupById = new Map((groups.data ?? []).map((group) => [group.id, group]));
    return (channels.data ?? []).map((channel) => {
      const group = groupById.get(channel.group_id);
      const channelCapabilities = (capabilities.data ?? []).filter((capability) =>
        capability.channel_id === channel.id);
      return {
        id: channel.id,
        channel_group_id: channel.group_id,
        channel_group_name: group?.name,
        channel_group_enabled: group?.enabled ?? false,
        name: channel.name,
        enabled: channel.enabled,
        auto_disabled: channelCapabilities.length > 0 &&
          channelCapabilities.every((capability) => capability.auto_disabled),
      };
    });
  }, [channels.data, groups.data, capabilities.data]);

  const patch = (partial: Partial<FormState>) => {
    draft.markDirty();
    setState((prev) => ({ ...prev, ...partial }));
  };

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    const input: ApiKeyPolicyInput = parsed.data;
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("Policy created"));
        draft.markSaved();
        draft.navigate(returnTo, { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        draft.markSaved();
        toast.success(t("Policy updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This policy was changed elsewhere. Reloading."));
      } else {
        toast.error(error instanceof Error ? error.message : t("Save failed"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)?.message;
    return message ? t(message) : undefined;
  };
  const targetError = fieldError("allowed_group_ids") ?? fieldError("allowed_channel_ids");

  return (
    <AdminDetailShell
      title={isNew ? t("New API key policy") : state.name || t("Policy")}
      description={t("Controls which channel groups and channels users may assign to API keys.")}
      backPath={returnTo}
      navigationGuard={draft.navigationGuard}
      isLoading={isLoading || groups.isLoading || channels.isLoading || capabilities.isLoading}
      error={error ?? groups.error ?? channels.error ?? capabilities.error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Policy")}</CardTitle>
              <CardDescription>
                {t("Updated")} {formatRelative(data.data.updated_at)}.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4">
                <DetailField
                  label={t("Enabled")}
                  value={<StatusBadge value={data.data.enabled} />}
                />
                <DetailField
                  label={t("Channel groups")}
                  value={data.data.allowed_group_ids.length}
                />
                <DetailField
                  label={t("Individual channels")}
                  value={data.data.allowed_channel_ids.length}
                />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? t("Create policy") : t("Edit policy")}</CardTitle>
            <CardDescription>
              {t("Users choose each key's targets and limits from these permitted resources.")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="flex flex-col gap-4" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
              <FieldGroup className="grid gap-5 xl:grid-cols-2">
                <Field data-invalid={Boolean(fieldError("name"))}>
                  <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                  <Input
                    id="name"
                    value={state.name}
                    onChange={(event) => patch({ name: event.target.value })}
                    aria-invalid={Boolean(fieldError("name"))}
                  />
                  {fieldError("name") ? (
                    <FieldError>{fieldError("name")}</FieldError>
                  ) : null}
                </Field>
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="policy_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="policy_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
                <RoutingTargetFields
                  className="xl:col-span-2"
                  groups={targetGroups}
                  channels={targetChannels}
                  selectedGroupIds={state.allowed_group_ids}
                  selectedChannelIds={state.allowed_channel_ids}
                  onChange={(allowedGroupIds, allowedChannelIds) =>
                    patch({
                      allowed_group_ids: allowedGroupIds,
                      allowed_channel_ids: allowedChannelIds,
                    })
                  }
                  error={targetError}
                />
              </FieldGroup>
              <Button type="submit" className="self-start" disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create policy") : t("Save policy")}
              </Button>
            </form>
          </CardContent>
        </Card>
      }
    />
  );
}
