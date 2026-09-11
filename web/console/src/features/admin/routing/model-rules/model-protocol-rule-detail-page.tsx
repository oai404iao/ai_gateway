import { useEffect, useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { useI18n } from "@/app/i18n";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ModelProtocolRuleInput } from "@/api/types";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
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
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import {
  useChannelGroups,
  useChannels,
  useModelProtocolRule,
  useModelRule,
  useUpdateModelProtocolRule,
} from "@/features/admin/api";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { ConfigurationSaveBar } from "@/features/admin/model-setup/configuration-save-bar";
import {
  adminPath,
  safeAdminReturnPath,
} from "@/features/admin/model-setup/model-setup-navigation";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";
import { apiFormatLabel } from "@/lib/permissions";
import { ModelRuleTierEditor } from "./model-rule-tier-editor";

const channelSchema = z.object({
  channel_id: z.string().min(1),
  upstream_model: z.string().min(1).nullable(),
  weight: z.number().int().min(1).max(2_147_483_647),
});

const groupSchema = z
  .object({
    channel_group_id: z.string().min(1),
    channel_selection: z.enum(["all", "selected"]),
    upstream_model: z.string().min(1).nullable(),
    default_weight: z.number().int().min(1).max(2_147_483_647).nullable(),
    channels: z.array(channelSchema),
  })
  .superRefine((value, context) => {
    if (
      value.channel_selection === "all" &&
      (value.upstream_model === null || value.default_weight === null)
    ) {
      context.addIssue({
        code: "custom",
        path: ["upstream_model"],
        message: "Choose an upstream model and default weight for this group.",
      });
    }
    if (
      value.channel_selection === "all" &&
      value.channels.some((channel) => channel.upstream_model !== null)
    ) {
      context.addIssue({
        code: "custom",
        path: ["channels"],
        message: "Group weight overrides inherit the group upstream model.",
      });
    }
    if (
      value.channel_selection === "selected" &&
      (value.upstream_model !== null ||
        value.default_weight !== null ||
        value.channels.length === 0 ||
        value.channels.some((channel) => channel.upstream_model === null))
    ) {
      context.addIssue({
        code: "custom",
        path: ["channels"],
        message: "Every selected channel needs an upstream model and weight.",
      });
    }
    const ids = new Set<string>();
    value.channels.forEach((channel, index) => {
      if (ids.has(channel.channel_id)) {
        context.addIssue({
          code: "custom",
          path: ["channels", index, "channel_id"],
          message: "A channel can appear only once in a group target.",
        });
      }
      ids.add(channel.channel_id);
    });
  });

const tierSchema = z.object({
  priority: z.number().int().min(0).max(2_147_483_647),
  selection_strategy: z.enum([
    "weighted_random",
    "weighted_round_robin",
  ]),
  channel_groups: z.array(groupSchema).min(1),
});

const schema = z
  .object({
    description: z.string().nullable(),
    routing_tiers: z.array(tierSchema),
    enabled: z.boolean(),
  })
  .superRefine((value, context) => {
    if (value.enabled && value.routing_tiers.length === 0) {
      context.addIssue({
        code: "custom",
        path: ["routing_tiers"],
        message: "Add a priority tier before enabling this protocol.",
      });
    }
    const priorities = new Set<number>();
    const groupIds = new Set<string>();
    value.routing_tiers.forEach((tier, tierIndex) => {
      if (priorities.has(tier.priority)) {
        context.addIssue({
          code: "custom",
          path: ["routing_tiers", tierIndex, "priority"],
          message: "Tier priorities must be unique.",
        });
      }
      priorities.add(tier.priority);
      tier.channel_groups.forEach((group, groupIndex) => {
        if (groupIds.has(group.channel_group_id)) {
          context.addIssue({
            code: "custom",
            path: [
              "routing_tiers",
              tierIndex,
              "channel_groups",
              groupIndex,
              "channel_group_id",
            ],
            message: "A channel group can appear only once in a protocol.",
          });
        }
        groupIds.add(group.channel_group_id);
      });
    });
  });

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  description: null,
  routing_tiers: [],
  enabled: false,
};

export function ModelProtocolRuleDetailPage() {
  const { id = "", protocolId = "" } = useParams();
  const [searchParams] = useSearchParams();
  const { t } = useI18n();
  const parent = useModelRule(id);
  const protocol = useModelProtocolRule(id, protocolId);
  const groups = useChannelGroups();
  const channels = useChannels();
  const update = useUpdateModelProtocolRule(id, protocolId);
  const returnTo = safeAdminReturnPath(
    searchParams.get("returnTo"),
    "/admin/routing/model-rules",
  );
  const parentPath = adminPath(`/admin/routing/model-rules/${id}`, {
    returnTo,
  });
  const [state, setState] = useState<FormState>(empty);
  const [validation, setValidation] = useState<z.ZodError | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const {
    dirty,
    markDirty,
    markSaved,
    navigate,
    navigationGuard,
  } = useConfigurationDraft(submitting);

  useEffect(() => {
    if (!protocol.data) return;
    setState({
      description: protocol.data.data.description,
      routing_tiers: protocol.data.data.routing_tiers,
      enabled: protocol.data.data.enabled,
    });
  }, [protocol.data]);

  const apiFormat = protocol.data?.data.api_format;
  const targetGroups = useMemo(
    () =>
      (groups.data ?? []).filter((group) => group.api_format === apiFormat),
    [apiFormat, groups.data],
  );
  const targetChannels = useMemo(
    () =>
      (channels.data ?? []).filter(
        (channel) => channel.api_format === apiFormat,
      ),
    [apiFormat, channels.data],
  );

  const patch = (partial: Partial<FormState>) => {
    markDirty();
    setState((current) => ({ ...current, ...partial }));
  };
  const fieldError = (path: string | Array<string | number>) => {
    const normalized = Array.isArray(path) ? path.join(".") : path;
    const message = validation?.issues.find(
      (issue) => issue.path.join(".") === normalized,
    )?.message;
    return message ? t(message) : undefined;
  };
  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    try {
      await update.mutateAsync({
        input: parsed.data satisfies ModelProtocolRuleInput,
        ifMatch: protocol.etag,
      });
      markSaved();
      toast.success(t("Protocol rule updated"));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This protocol was changed elsewhere. Reloading."));
        markSaved();
        await protocol.refetch();
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const protocolData = protocol.data?.data;
  const parentData = parent.data?.data;
  return (
    <AdminDetailShell
      configurationLens="routes"
      navigationGuard={navigationGuard}
      saving={submitting}
      onBack={() => navigate(parentPath)}
      actionBar={
        <ConfigurationSaveBar
          dirty={dirty}
          saving={submitting}
          onCancel={() => navigate(parentPath)}
        >
          <Button onClick={() => void submit()} disabled={submitting}>
            {submitting ? <Spinner data-icon="inline-start" /> : null}
            {t("Save protocol")}
          </Button>
        </ConfigurationSaveBar>
      }
      title={
        parentData && protocolData
          ? `${parentData.model_display_name} · ${apiFormatLabel(protocolData.api_format)}`
          : t("Protocol rule")
      }
      description={t(
        "Configure ordered channel targets and choose the upstream model at the target that owns it.",
      )}
      backPath={parentPath}
      backLabel={t("Back to model rule")}
      isLoading={
        parent.isLoading ||
        protocol.isLoading ||
        groups.isLoading ||
        channels.isLoading
      }
      error={
        parent.error ?? protocol.error ?? groups.error ?? channels.error
      }
      hasData={Boolean(parentData && protocolData)}
      detailCard={
        protocolData ? (
          <Card size="sm">
            <CardHeader>
              <CardTitle>{apiFormatLabel(protocolData.api_format)}</CardTitle>
              <CardDescription>
                {parentData?.client_model ?? ""}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="flex flex-col gap-4">
                <DetailField
                  label={t("Routing status")}
                  value={<StatusBadge value={protocolData.routing_status} />}
                />
                <DetailField
                  label={t("Active channels")}
                  value={`${protocolData.active_channel_count} / ${protocolData.target_channel_count}`}
                />
                <DetailField
                  label={t("Model-capable channels")}
                  value={protocolData.model_capable_channel_count}
                />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        protocolData ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Forwarding rules")}</CardTitle>
              <CardDescription>
                {t(
                  "Lower priority numbers run first. A disabled protocol may be saved without any tiers.",
                )}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="protocol_description">
                    {t("Description")}
                  </FieldLabel>
                  <Input
                    id="protocol_description"
                    value={state.description ?? ""}
                    onChange={(event) =>
                      patch({ description: event.target.value || null })
                    }
                  />
                </Field>
                <ModelRuleTierEditor
                  value={state.routing_tiers}
                  groups={targetGroups}
                  channels={targetChannels}
                  onChange={(routingTiers) =>
                    patch({ routing_tiers: routingTiers })
                  }
                  errorFor={(path) => fieldError(path)}
                />
                {fieldError("routing_tiers") ? (
                  <FieldError>{fieldError("routing_tiers")}</FieldError>
                ) : null}
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="protocol_enabled">
                    {t("Enabled")}
                  </FieldLabel>
                  <Switch
                    id="protocol_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) =>
                      patch({ enabled: Boolean(checked) })
                    }
                  />
                </Field>
              </FieldGroup>
            </CardContent>
          </Card>
        ) : null
      }
    />
  );
}
