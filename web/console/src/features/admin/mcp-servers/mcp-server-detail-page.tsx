import { useEffect, useMemo, useState } from "react";
import {
  Controller,
  useForm,
  type UseFormReturn,
} from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { useNavigate, useParams } from "react-router";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
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
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { StringListField } from "@/components/shared/string-list-field";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import {
  useCreateMcpServer,
  useDeleteMcpServer,
  useMcpServer,
  useModelRules,
  useUpdateMcpServer,
} from "@/features/admin/api";
import {
  defaultMcpServerFormValues,
  mcpContextSizeLabel,
  mcpExternalAccessLabel,
  mcpImageBackgroundLabel,
  mcpImageQualityLabel,
  mcpKindApiFormat,
  mcpKindLabel,
  mcpServerCreateInput,
  mcpServerFormSchema,
  mcpServerFormValues,
  mcpServerInput,
  mcpToolName,
  type McpServerFormValues,
} from "@/features/admin/mcp-servers/mcp-server-form";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type {
  McpImageBackground,
  McpImageQuality,
  McpSearchContextSize,
  McpSearchExternalWebAccess,
  McpServerKind,
} from "@/api/types";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const NONE = "__none__";

export function McpServerDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const detail = useMcpServer(id);
  const modelRules = useModelRules();
  const create = useCreateMcpServer();
  const update = useUpdateMcpServer(id);
  const remove = useDeleteMcpServer(id);
  const { t } = useI18n();
  const [deleteOpen, setDeleteOpen] = useState(false);
  const form = useForm<McpServerFormValues>({
    resolver: zodResolver(mcpServerFormSchema),
    defaultValues: defaultMcpServerFormValues,
  });
  const kind = form.watch("kind");
  const expectedApiFormat = mcpKindApiFormat(kind);
  const server = detail.data?.data;

  useEffect(() => {
    if (server) {
      form.reset(mcpServerFormValues(server));
    }
  }, [form, server]);

  const compatibleRules = useMemo(
    () =>
      (modelRules.data ?? [])
        .filter((rule) => rule.api_format === expectedApiFormat)
        .sort((left, right) =>
          left.client_model.localeCompare(right.client_model),
        ),
    [expectedApiFormat, modelRules.data],
  );

  const submit = form.handleSubmit(async (values) => {
    const selectedRule = modelRules.data?.find(
      (rule) => rule.id === values.model_rule_id,
    );
    if (!selectedRule || selectedRule.api_format !== mcpKindApiFormat(values.kind)) {
      form.setError("model_rule_id", {
        message: "Pick a compatible model rule.",
      });
      return;
    }
    try {
      if (isNew) {
        await create.mutateAsync(mcpServerCreateInput(values));
        toast.success(t("MCP server created"));
        navigate("/admin/mcp-servers", { replace: true });
      } else {
        await update.mutateAsync({
          input: mcpServerInput(values),
          ifMatch: detail.etag,
        });
        toast.success(t("MCP server updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.code === "mcp_server_slug_conflict") {
        toast.error(
          t("This MCP endpoint slug is already reserved, including by a deleted server."),
        );
      } else if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This MCP server was changed elsewhere. Reloading."));
        await detail.refetch();
      } else if (error instanceof ApiError && error.isValidation) {
        toast.error(
          t("Check the server kind, model rule, route eligibility, and tool settings."),
        );
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
      }
    }
  });

  const deleteServer = async () => {
    setDeleteOpen(false);
    try {
      await remove.mutateAsync({ ifMatch: detail.etag });
      toast.success(t("MCP server deleted"));
      navigate("/admin/mcp-servers", { replace: true });
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This MCP server was changed elsewhere. Reloading."));
        await detail.refetch();
      } else {
        toast.error(
          controlPlaneMutationErrorMessage(error, t("Delete failed")),
        );
      }
    }
  };

  const pending =
    form.formState.isSubmitting ||
    create.isPending ||
    update.isPending ||
    remove.isPending;
  const errorMessage = (error?: { message?: string }) =>
    error?.message ? t(error.message) : undefined;

  return (
    <>
      <AdminDetailShell
        title={
          isNew
            ? t("New MCP server")
            : form.watch("name") || t("MCP server")
        }
        description={t(
          "Expose one managed MCP tool through an existing Gateway model rule.",
        )}
        backPath="/admin/mcp-servers"
        backLabel={t("Back to MCP servers")}
        isLoading={detail.isLoading || modelRules.isLoading}
        error={detail.error ?? modelRules.error}
        hasData={isNew || Boolean(server)}
        detailCard={
          !isNew && server ? (
            <Card>
              <CardHeader>
                <CardTitle>{server.name}</CardTitle>
                <CardDescription className="font-mono">
                  /mcp/{server.slug}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
                  <DetailField
                    label={t("Kind")}
                    value={
                      <StatusBadge
                        value={server.kind}
                        label={t(mcpKindLabel(server.kind))}
                        variant="info"
                      />
                    }
                  />
                  <DetailField
                    label={t("Tool")}
                    value={mcpToolName(server.kind)}
                    mono
                  />
                  <DetailField
                    label={t("Client model")}
                    value={server.client_model}
                    mono
                  />
                  <DetailField
                    label={t("API format")}
                    value={apiFormatLabel(server.api_format)}
                  />
                  <DetailField
                    label={t("Settings version")}
                    value={server.settings_version}
                  />
                  <DetailField
                    label={t("Enabled")}
                    value={<StatusBadge value={server.enabled} />}
                  />
                </dl>
              </CardContent>
            </Card>
          ) : null
        }
        editCard={
          <form onSubmit={submit} className="flex flex-col gap-6">
            <Card>
              <CardHeader>
                <CardTitle>
                  {isNew ? t("Create MCP server") : t("Edit MCP server")}
                </CardTitle>
                <CardDescription>
                  {t(
                    "The binary must include the mcp-server feature and the MCP transport must be enabled in System settings before public endpoints are reachable.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup className="grid gap-5 xl:grid-cols-2">
                  {isNew ? (
                    <>
                      <Field
                        data-invalid={Boolean(form.formState.errors.slug)}
                      >
                        <FieldLabel htmlFor="mcp_slug">
                          {t("Endpoint slug")}
                        </FieldLabel>
                        <Input
                          id="mcp_slug"
                          {...form.register("slug", {
                            onChange: (event) => {
                              event.target.value =
                                event.target.value.toLowerCase();
                            },
                          })}
                          placeholder="search"
                          aria-invalid={Boolean(form.formState.errors.slug)}
                        />
                        <FieldDescription>
                          {t("Creates the immutable public path /mcp/{slug}.", {
                            slug: form.watch("slug") || "search",
                          })}
                        </FieldDescription>
                        <FieldError>
                          {errorMessage(form.formState.errors.slug)}
                        </FieldError>
                      </Field>
                      <Controller
                        control={form.control}
                        name="kind"
                        render={({ field }) => (
                          <Field>
                            <FieldLabel htmlFor="mcp_kind">
                              {t("Kind")}
                            </FieldLabel>
                            <Select
                              value={field.value}
                              onValueChange={(value) => {
                                const next = value as McpServerKind;
                                field.onChange(next);
                                form.setValue("model_rule_id", "", {
                                  shouldValidate: false,
                                });
                              }}
                            >
                              <SelectTrigger
                                id="mcp_kind"
                                aria-label={t("Kind")}
                              >
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                <SelectGroup>
                                  <SelectItem value="web_search">
                                    {t("Web search")}
                                  </SelectItem>
                                  <SelectItem value="image">
                                    {t("Images")}
                                  </SelectItem>
                                </SelectGroup>
                              </SelectContent>
                            </Select>
                            <FieldDescription>
                              {t(
                                "Kind selects a statically compiled tool and cannot be changed later.",
                              )}
                            </FieldDescription>
                          </Field>
                        )}
                      />
                    </>
                  ) : null}
                  <Field
                    data-invalid={Boolean(form.formState.errors.name)}
                  >
                    <FieldLabel htmlFor="mcp_name">{t("Name")}</FieldLabel>
                    <Input
                      id="mcp_name"
                      {...form.register("name")}
                      aria-invalid={Boolean(form.formState.errors.name)}
                    />
                    <FieldError>
                      {errorMessage(form.formState.errors.name)}
                    </FieldError>
                  </Field>
                  <Field
                    className="xl:col-span-2"
                    data-invalid={Boolean(form.formState.errors.description)}
                  >
                    <FieldLabel htmlFor="mcp_description">
                      {t("Description")}
                    </FieldLabel>
                    <Textarea
                      id="mcp_description"
                      rows={3}
                      {...form.register("description")}
                      aria-invalid={Boolean(
                        form.formState.errors.description,
                      )}
                    />
                    <FieldDescription>
                      {t("Optional instructions shown to MCP clients during discovery.")}
                    </FieldDescription>
                    <FieldError>
                      {errorMessage(form.formState.errors.description)}
                    </FieldError>
                  </Field>
                  <Controller
                    control={form.control}
                    name="model_rule_id"
                    render={({ field, fieldState }) => (
                      <Field data-invalid={Boolean(fieldState.error)}>
                        <FieldLabel htmlFor="mcp_model_rule">
                          {t("Model rule")}
                        </FieldLabel>
                        <Select
                          value={field.value || NONE}
                          onValueChange={(value) =>
                            field.onChange(value === NONE ? "" : value)
                          }
                        >
                          <SelectTrigger
                            id="mcp_model_rule"
                            aria-label={t("Model rule")}
                            aria-invalid={Boolean(fieldState.error)}
                          >
                            <SelectValue
                              placeholder={t("Pick a compatible model rule")}
                            />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectGroup>
                              <SelectItem value={NONE}>{t("None")}</SelectItem>
                              {compatibleRules.map((rule) => (
                                <SelectItem key={rule.id} value={rule.id}>
                                  {rule.client_model} → {rule.upstream_model}
                                  {!rule.enabled ? ` (${t("Disabled")})` : ""}
                                </SelectItem>
                              ))}
                            </SelectGroup>
                          </SelectContent>
                        </Select>
                        <FieldDescription>
                          {t(
                            "An enabled server requires an enabled {format} rule with at least one eligible channel.",
                            { format: apiFormatLabel(expectedApiFormat) },
                          )}
                        </FieldDescription>
                        <FieldError>
                          {errorMessage(fieldState.error)}
                        </FieldError>
                      </Field>
                    )}
                  />
                  <Controller
                    control={form.control}
                    name="enabled"
                    render={({ field }) => (
                      <Field orientation="horizontal">
                        <FieldLabel htmlFor="mcp_enabled">
                          {t("Enabled")}
                        </FieldLabel>
                        <Switch
                          id="mcp_enabled"
                          checked={field.value}
                          onCheckedChange={(checked) =>
                            field.onChange(Boolean(checked))
                          }
                        />
                      </Field>
                    )}
                  />
                </FieldGroup>
              </CardContent>
            </Card>

            {kind === "web_search" ? (
              <SearchSettingsCard form={form} />
            ) : (
              <ImageSettingsCard form={form} />
            )}

            <Button className="self-start" type="submit" disabled={pending}>
              {form.formState.isSubmitting ||
              create.isPending ||
              update.isPending ? (
                <Spinner data-icon="inline-start" />
              ) : null}
              {isNew ? t("Create MCP server") : t("Save MCP server")}
            </Button>
          </form>
        }
        dangerZone={
          !isNew && server ? (
            <div className="flex flex-col items-start gap-4">
              <Alert>
                <AlertTitle>{t("Endpoint slug remains reserved")}</AlertTitle>
                <AlertDescription>
                  {t(
                    "Deleting removes this endpoint from the runtime registry, but the slug cannot be reused.",
                  )}
                </AlertDescription>
              </Alert>
              <Button
                variant="destructive"
                disabled={pending}
                onClick={() => setDeleteOpen(true)}
              >
                {remove.isPending ? (
                  <Spinner data-icon="inline-start" />
                ) : null}
                {t("Delete MCP server")}
              </Button>
            </div>
          ) : undefined
        }
      />
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={t("Delete MCP server?")}
        description={t(
          "This removes /mcp/{slug} from the runtime registry. The reserved slug cannot be reused.",
          { slug: server?.slug ?? "" },
        )}
        confirmLabel={t("Delete MCP server")}
        destructive
        onConfirm={() => void deleteServer()}
      />
    </>
  );
}

function SearchSettingsCard({
  form,
}: {
  form: UseFormReturn<McpServerFormValues>;
}) {
  const { t } = useI18n();
  const errorMessage = (error?: { message?: string }) =>
    error?.message ? t(error.message) : undefined;
  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Web search policy")}</CardTitle>
        <CardDescription>
          {t(
            "Control search freshness, context size, domain policy, and bounded output tokens.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <FieldGroup className="grid gap-5 xl:grid-cols-2">
          <Controller
            control={form.control}
            name="external_web_access"
            render={({ field }) => (
              <Field>
                <FieldLabel htmlFor="mcp_external_web_access">
                  {t("External web access")}
                </FieldLabel>
                <Select
                  value={field.value}
                  onValueChange={(value) =>
                    field.onChange(value as McpSearchExternalWebAccess)
                  }
                >
                  <SelectTrigger
                    id="mcp_external_web_access"
                    aria-label={t("External web access")}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(["cached", "indexed", "live"] as const).map(
                        (value) => (
                          <SelectItem key={value} value={value}>
                            {t(mcpExternalAccessLabel(value))}
                          </SelectItem>
                        ),
                      )}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            )}
          />
          <Controller
            control={form.control}
            name="search_context_size"
            render={({ field }) => (
              <Field>
                <FieldLabel htmlFor="mcp_search_context_size">
                  {t("Search context size")}
                </FieldLabel>
                <Select
                  value={field.value}
                  onValueChange={(value) =>
                    field.onChange(value as McpSearchContextSize)
                  }
                >
                  <SelectTrigger
                    id="mcp_search_context_size"
                    aria-label={t("Search context size")}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(["low", "medium", "high"] as const).map((value) => (
                        <SelectItem key={value} value={value}>
                          {t(mcpContextSizeLabel(value))}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            )}
          />
          <Controller
            control={form.control}
            name="allowed_domains"
            render={({ field, fieldState }) => (
              <StringListField
                id="mcp_allowed_domains"
                label={t("Allowed domains")}
                description={t(
                  "Optional allowlist. Enter bare DNS names, one per line.",
                )}
                value={field.value}
                onChange={field.onChange}
                placeholder={"example.com\ndocs.example.com"}
                error={errorMessage(fieldState.error)}
              />
            )}
          />
          <Controller
            control={form.control}
            name="blocked_domains"
            render={({ field, fieldState }) => (
              <StringListField
                id="mcp_blocked_domains"
                label={t("Blocked domains")}
                description={t(
                  "These domains are always excluded and cannot also be allowed.",
                )}
                value={field.value}
                onChange={field.onChange}
                placeholder={"ads.example.com\ninternal.example.com"}
                error={errorMessage(fieldState.error)}
              />
            )}
          />
          <FieldSet className="xl:col-span-2">
            <FieldLegend>{t("Maximum output tokens")}</FieldLegend>
            <FieldDescription>
              {t(
                "The selected response length maps to ordered short, medium, and long limits.",
              )}
            </FieldDescription>
            <FieldGroup className="grid gap-5 sm:grid-cols-3">
              {(
                [
                  ["max_output_tokens_short", "Short"],
                  ["max_output_tokens_medium", "Medium"],
                  ["max_output_tokens_long", "Long"],
                ] as const
              ).map(([name, label]) => (
                <Field
                  key={name}
                  data-invalid={Boolean(form.formState.errors[name])}
                >
                  <FieldLabel htmlFor={`mcp_${name}`}>
                    {t(label)}
                  </FieldLabel>
                  <Input
                    id={`mcp_${name}`}
                    type="number"
                    min={1}
                    max={100_000}
                    {...form.register(name, { valueAsNumber: true })}
                    aria-invalid={Boolean(form.formState.errors[name])}
                  />
                  <FieldError>
                    {errorMessage(form.formState.errors[name])}
                  </FieldError>
                </Field>
              ))}
            </FieldGroup>
          </FieldSet>
        </FieldGroup>
      </CardContent>
    </Card>
  );
}

function ImageSettingsCard({
  form,
}: {
  form: UseFormReturn<McpServerFormValues>;
}) {
  const { t } = useI18n();
  const errorMessage = (error?: { message?: string }) =>
    error?.message ? t(error.message) : undefined;
  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Image defaults")}</CardTitle>
        <CardDescription>
          {t(
            "These values are fixed by the MCP instance and cannot be overridden by callers.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <FieldGroup className="grid gap-5 xl:grid-cols-3">
          <Controller
            control={form.control}
            name="image_background"
            render={({ field }) => (
              <Field>
                <FieldLabel htmlFor="mcp_image_background">
                  {t("Background")}
                </FieldLabel>
                <Select
                  value={field.value}
                  onValueChange={(value) =>
                    field.onChange(value as McpImageBackground)
                  }
                >
                  <SelectTrigger
                    id="mcp_image_background"
                    aria-label={t("Background")}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(["auto", "opaque", "transparent"] as const).map(
                        (value) => (
                          <SelectItem key={value} value={value}>
                            {t(mcpImageBackgroundLabel(value))}
                          </SelectItem>
                        ),
                      )}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            )}
          />
          <Controller
            control={form.control}
            name="image_quality"
            render={({ field }) => (
              <Field>
                <FieldLabel htmlFor="mcp_image_quality">
                  {t("Quality")}
                </FieldLabel>
                <Select
                  value={field.value}
                  onValueChange={(value) =>
                    field.onChange(value as McpImageQuality)
                  }
                >
                  <SelectTrigger
                    id="mcp_image_quality"
                    aria-label={t("Quality")}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {(["auto", "low", "medium", "high"] as const).map(
                        (value) => (
                          <SelectItem key={value} value={value}>
                            {t(mcpImageQualityLabel(value))}
                          </SelectItem>
                        ),
                      )}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            )}
          />
          <Field data-invalid={Boolean(form.formState.errors.image_size)}>
            <FieldLabel htmlFor="mcp_image_size">{t("Size")}</FieldLabel>
            <Input
              id="mcp_image_size"
              {...form.register("image_size")}
              placeholder="auto"
              aria-invalid={Boolean(form.formState.errors.image_size)}
            />
            <FieldDescription>
              {t("Use auto or WIDTHxHEIGHT; each dimension must be 64-8192.")}
            </FieldDescription>
            <FieldError>
              {errorMessage(form.formState.errors.image_size)}
            </FieldError>
          </Field>
        </FieldGroup>
      </CardContent>
    </Card>
  );
}
