import { useEffect, useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";
import { ConfigurationSaveBar } from "@/features/admin/model-setup/configuration-save-bar";
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
  RequestCompression,
} from "@/api/types";
import {
  API_FORMATS,
  CONNECTOR_KINDS,
  REQUEST_COMPRESSIONS,
  apiFormatLabel,
  connectorKindLabel,
  requestCompressionLabel,
} from "@/lib/permissions";
import { useI18n } from "@/app/i18n";
import { safeAdminReturnPath } from "@/features/admin/model-setup/model-setup-navigation";

const schema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  api_format: z.enum(["open_ai_chat_completions", "open_ai_responses", "open_ai_images"]),
  connector_kind: z.enum(["openai_compatible", "codex_oauth"]),
  request_compression: z.enum(["default", "zstd"]),
  enabled: z.boolean(),
  status_statistics_enabled: z.boolean(),
  sharing_only: z.boolean(),
});

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  api_format: "open_ai_chat_completions",
  connector_kind: "openai_compatible",
  request_compression: "default",
  enabled: true,
  status_statistics_enabled: false,
  sharing_only: false,
};

export function ChannelGroupDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const [searchParams] = useSearchParams();
  const returnTo = safeAdminReturnPath(
    searchParams.get("returnTo"),
    "/admin/routing/channels",
  );
  const returnsToSetup = returnTo.startsWith("/admin/model-setup");
  const { data, etag, isLoading, error } = useChannelGroup(id);
  const create = useCreateChannelGroup();
  const update = useUpdateChannelGroup(id);
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [submitting, setSubmitting] = useState(false);
  const { dirty, markDirty, markSaved, navigate, navigationGuard } = useConfigurationDraft(submitting);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        api_format: data.data.api_format,
        connector_kind: data.data.connector_kind,
        request_compression: data.data.request_compression,
        enabled: data.data.enabled,
        status_statistics_enabled: data.data.status_statistics_enabled,
        sharing_only: data.data.sharing_only,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => {
    markDirty();
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
    const input: ChannelGroupInput = {
      name: parsed.data.name,
      api_format: parsed.data.api_format as ApiFormat,
      connector_kind: parsed.data.connector_kind as ConnectorKind,
      request_compression: parsed.data.request_compression as RequestCompression,
      enabled: parsed.data.enabled,
      status_statistics_enabled: parsed.data.status_statistics_enabled,
      sharing_only: parsed.data.sharing_only,
    };
    try {
      if (isNew) {
        await create.mutateAsync(input);
        markSaved();
        toast.success(t("Channel group created"));
        navigate(returnTo, { replace: true });
      } else {
        await update.mutateAsync({ input, ifMatch: etag });
        markSaved();
        toast.success(t("Channel group updated"));
        if (searchParams.has("returnTo")) navigate(returnTo, { replace: true });
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
      configurationLens="supply"
      navigationGuard={navigationGuard}
      saving={submitting}
      onBack={() => navigate(returnTo)}
      actionBar={
        <ConfigurationSaveBar dirty={dirty} saving={submitting} onCancel={() => navigate(returnTo)}>
          <Button onClick={submit} disabled={submitting}>
            {submitting ? <Spinner data-icon="inline-start" /> : null}
            {isNew ? t("Create group") : t("Save group")}
          </Button>
        </ConfigurationSaveBar>
      }
      title={isNew ? t("New channel group") : state.name || t("Channel group")}
      description={t("A same-format pool of upstream channels.")}
      backPath={returnTo}
      backLabel={t(returnsToSetup ? "Back to model setup" : "Back to channels")}
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
                <DetailField
                  label={t("Connector")}
                  value={connectorKindLabel(data.data.connector_kind)}
                />
                <DetailField
                  label={t("Request compression")}
                  value={requestCompressionLabel(data.data.request_compression)}
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
                        sharing_only: connector === "codex_oauth" && state.sharing_only,
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
                    onValueChange={(value) => {
                      const apiFormat = value as ApiFormat;
                      patch({
                        api_format: apiFormat,
                        request_compression:
                          apiFormat === "open_ai_responses"
                            ? state.request_compression
                            : "default",
                      });
                    }}
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
                <Field>
                  <FieldLabel htmlFor="request_compression">
                    {t("Request compression")}
                  </FieldLabel>
                  <Select
                    value={state.request_compression}
                    disabled={state.api_format !== "open_ai_responses"}
                    onValueChange={(value) =>
                      patch({ request_compression: value as RequestCompression })
                    }
                  >
                    <SelectTrigger id="request_compression">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {REQUEST_COMPRESSIONS.map((compression) => (
                          <SelectItem key={compression} value={compression}>
                            {requestCompressionLabel(compression)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                  {state.api_format !== "open_ai_responses" ? (
                    <FieldDescription>
                      {t(
                        "Request compression is available only for Responses channel groups.",
                      )}
                    </FieldDescription>
                  ) : null}
                </Field>
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="channel_group_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="channel_group_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
                {state.connector_kind === "codex_oauth" && <Field orientation="horizontal">
                  <FieldContent>
                    <FieldLabel htmlFor="channel_group_sharing_only">{t("Sharing only")}</FieldLabel>
                    <FieldDescription>{t("Requires a sharing seat for this Codex pool. Applies to both Responses and Images without enabling Images. Existing bound credentials stay protected when turned off.")}</FieldDescription>
                  </FieldContent>
                  <Switch id="channel_group_sharing_only" checked={state.sharing_only}
                    onCheckedChange={checked => patch({ sharing_only: Boolean(checked) })} />
                </Field>}
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
