import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import {
  ApiKeyTargetFields,
  type ApiKeyTargetChannel,
  type ApiKeyTargetGroup,
} from "@/components/shared/api-key-target-fields";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useApiKeyPolicy,
  useChannelGroups,
  useChannels,
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
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useApiKeyPolicy(id);
  const groups = useChannelGroups();
  const channels = useChannels();
  const create = useCreateApiKeyPolicy();
  const update = useUpdateApiKeyPolicy(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

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

  const targetGroups = useMemo<ApiKeyTargetGroup[]>(
    () =>
      (groups.data ?? []).map((group) => ({
        id: group.id,
        name: group.name,
        api_format: group.api_format,
        enabled: group.enabled,
      })),
    [groups.data],
  );
  const targetChannels = useMemo<ApiKeyTargetChannel[]>(
    () =>
      (channels.data ?? []).map((channel) => ({
        id: channel.id,
        channel_group_id: channel.channel_group_id,
        channel_group_name: groups.data?.find(
          (group) => group.id === channel.channel_group_id,
        )?.name,
        name: channel.name,
        api_format: channel.api_format,
        enabled: channel.enabled,
        auto_disabled: channel.auto_disabled,
      })),
    [channels.data, groups.data],
  );

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

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
        navigate("/admin/api-key-policies", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
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
      backPath="/admin/api-key-policies"
      backLabel={t("Back to policies")}
      isLoading={isLoading || groups.isLoading || channels.isLoading}
      error={error ?? groups.error ?? channels.error}
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
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-3">
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
            <div className="flex flex-col gap-4">
              <FieldGroup>
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
                <ApiKeyTargetFields
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
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="policy_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="policy_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create policy") : t("Save policy")}
              </Button>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
