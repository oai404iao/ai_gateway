import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldContent,
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
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  useChannelGroup,
  useCreateChannelGroup,
  useUpdateChannelGroup,
} from "@/features/admin/api";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type {
  ApiFormat,
  ChannelGroupInput,
  ConnectorKind,
  SelectionStrategy,
} from "@/api/types";
import {
  API_FORMATS,
  CONNECTOR_KINDS,
  SELECTION_STRATEGIES,
  apiFormatLabel,
  connectorKindLabel,
  selectionStrategyLabel,
} from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const schema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  api_format: z.enum(["open_ai_chat_completions", "open_ai_responses", "open_ai_images"]),
  connector_kind: z.enum(["openai_compatible", "codex_oauth"]),
  priority: z.number().int().min(0, "Priority must be zero or greater."),
  selection_strategy: z.enum(["weighted_random", "weighted_round_robin"]),
  enabled: z.boolean(),
  status_statistics_enabled: z.boolean(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  api_format: "open_ai_chat_completions",
  connector_kind: "openai_compatible",
  priority: 1,
  selection_strategy: "weighted_random",
  enabled: true,
  status_statistics_enabled: false,
};

export function ChannelGroupDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useChannelGroup(id);
  const create = useCreateChannelGroup();
  const update = useUpdateChannelGroup(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        api_format: data.data.api_format,
        connector_kind: data.data.connector_kind,
        priority: data.data.priority,
        selection_strategy: data.data.selection_strategy,
        enabled: data.data.enabled,
        status_statistics_enabled: data.data.status_statistics_enabled,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => setState((prev) => ({ ...prev, ...partial }));

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    const input: ChannelGroupInput = {
      name: parsed.data.name,
      api_format: parsed.data.api_format as ApiFormat,
      connector_kind: parsed.data.connector_kind as ConnectorKind,
      priority: parsed.data.priority,
      selection_strategy: parsed.data.selection_strategy as SelectionStrategy,
      enabled: parsed.data.enabled,
      status_statistics_enabled: parsed.data.status_statistics_enabled,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        toast.success(t("Channel group created"));
        navigate("/admin/routing/channels", { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Channel group updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This group was changed elsewhere. Reloading."));
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)?.message;
    return message ? t(message) : undefined;
  };

  return (
    <AdminDetailShell
      title={isNew ? t("New channel group") : state.name || t("Channel group")}
      description={t("A same-format pool of channels selected by priority and weight.")}
      backPath="/admin/routing/channels"
      backLabel={t("Back to channels")}
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        !isNew && data ? (
          <Card>
            <CardHeader>
              <CardTitle>{data.data.name}</CardTitle>
              <CardDescription>{apiFormatLabel(data.data.api_format)}</CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <DetailField label={t("Priority")} value={data.data.priority} />
                <DetailField
                  label={t("Connector")}
                  value={connectorKindLabel(data.data.connector_kind)}
                />
                <DetailField
                  label={t("Strategy")}
                  value={selectionStrategyLabel(data.data.selection_strategy)}
                />
                <DetailField
                  label={t("Enabled")}
                  value={<StatusBadge value={data.data.enabled} />}
                />
                <DetailField
                  label={t("Status monitoring")}
                  value={<StatusBadge value={data.data.status_statistics_enabled} />}
                />
              </dl>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? t("Create group") : t("Edit group")}</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup className="grid gap-5 xl:grid-cols-2">
                <Field data-invalid={Boolean(fieldError("name"))}>
                  <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                  <Input
                    id="name"
                    value={state.name}
                    onChange={(event) => patch({ name: event.target.value })}
                    aria-invalid={Boolean(fieldError("name"))}
                  />
                  {fieldError("name") ? <FieldError>{fieldError("name")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel>{t("Connector")}</FieldLabel>
                  <Select
                    value={state.connector_kind}
                    disabled={!isNew}
                    onValueChange={(value) => {
                      const connector = value as ConnectorKind;
                      patch({
                        connector_kind: connector,
                        api_format:
                          connector === "codex_oauth"
                            ? "open_ai_responses"
                            : state.api_format,
                      });
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {CONNECTOR_KINDS.map((connector) => (
                          <SelectItem key={connector} value={connector}>
                            {connectorKindLabel(connector)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field>
                  <FieldLabel>{t("API format")}</FieldLabel>
                  <Select
                    value={state.api_format}
                    disabled={state.connector_kind === "codex_oauth"}
                    onValueChange={(value) => patch({ api_format: value as ApiFormat })}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {API_FORMATS.map((format) => (
                          <SelectItem key={format} value={format}>
                            {apiFormatLabel(format)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  {isNew && state.connector_kind === "codex_oauth" ? (
                    <FieldDescription>
                      {t(
                        "A disabled Images group is created with the Responses group so credentials can be shared without granting Images access.",
                      )}
                    </FieldDescription>
                  ) : null}
                </Field>
                <Field data-invalid={Boolean(fieldError("priority"))}>
                  <FieldLabel htmlFor="priority">{t("Priority")}</FieldLabel>
                  <Input
                    id="priority"
                    type="number"
                    min={0}
                    value={state.priority}
                    onChange={(event) =>
                      patch({ priority: Math.max(0, Number(event.target.value) || 0) })
                    }
                    aria-invalid={Boolean(fieldError("priority"))}
                  />
                  {fieldError("priority") ? (
                    <FieldError>{fieldError("priority")}</FieldError>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel>{t("Selection strategy")}</FieldLabel>
                  <Select
                    value={state.selection_strategy}
                    onValueChange={(value) =>
                      patch({ selection_strategy: value as SelectionStrategy })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {SELECTION_STRATEGIES.map((strategy) => (
                          <SelectItem key={strategy} value={strategy}>
                            {selectionStrategyLabel(strategy)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="channel_group_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="channel_group_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
                <Field orientation="horizontal">
                  <FieldContent>
                    <FieldLabel htmlFor="channel_group_status_statistics_enabled">
                      {t("Status monitoring")}
                    </FieldLabel>
                    <FieldDescription>
                      {t(
                        "Include this channel group in the channel group status report.",
                      )}
                    </FieldDescription>
                  </FieldContent>
                  <Switch
                    id="channel_group_status_statistics_enabled"
                    checked={state.status_statistics_enabled}
                    onCheckedChange={(checked) =>
                      patch({ status_statistics_enabled: Boolean(checked) })
                    }
                  />
                </Field>
              </FieldGroup>
              <Button className="self-start" onClick={submit} disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                {isNew ? t("Create group") : t("Save group")}
              </Button>
              {!isNew && data?.data.connector_kind === "codex_oauth" ? (
                <Button
                  className="self-start"
                  variant="outline"
                  onClick={() => navigate(`/admin/providers/codex-oauth/${data.data.id}`)}
                >
                  {t("Manage Codex credentials")}
                </Button>
              ) : null}
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}
