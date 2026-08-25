import { useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import {
  ArrowRight,
  Boxes,
  Calculator,
  Check,
  CircleAlert,
  Copy,
  Layers3,
  Network,
  Pencil,
  Plus,
  Route,
  Search,
  Sparkles,
  Workflow,
  type LucideIcon,
} from "lucide-react";
import type {
  ChannelGroupView,
  ChannelView,
  ControlPlaneModel,
  ModelRuleView,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Progress } from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import {
  useChannelGroups,
  useChannels,
  useModelRules,
  useModels,
} from "@/features/admin/api";
import {
  MODEL_SETUP_PATH,
  adminPath,
} from "@/features/admin/model-setup/model-setup-navigation";
import { formatDecimal } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";

type SetupView = "overview" | "suppliers" | "models" | "rules";

const SETUP_VIEWS: SetupView[] = [
  "overview",
  "suppliers",
  "models",
  "rules",
];

interface PickerItem {
  id: string;
  title: string;
  description: string;
  searchText: string;
}

function CopyPickerDialog({
  open,
  title,
  description,
  emptyTitle,
  items,
  onOpenChange,
  onSelect,
}: {
  open: boolean;
  title: string;
  description: string;
  emptyTitle: string;
  items: PickerItem[];
  onOpenChange: (open: boolean) => void;
  onSelect: (id: string) => void;
}) {
  const { t } = useI18n();
  const [search, setSearch] = useState("");
  const normalizedSearch = search.trim().toLowerCase();
  const filteredItems = items.filter(
    (item) =>
      !normalizedSearch ||
      item.searchText.toLowerCase().includes(normalizedSearch),
  );

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) setSearch("");
        onOpenChange(nextOpen);
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <InputGroup>
          <InputGroupAddon>
            <Search aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            type="search"
            value={search}
            aria-label={t("Search")}
            placeholder={t("Search by name, model, or endpoint")}
            onChange={(event) => setSearch(event.target.value)}
          />
        </InputGroup>
        {filteredItems.length > 0 ? (
          <div className="flex max-h-80 flex-col gap-2 overflow-y-auto">
            {filteredItems.map((item) => (
              <Button
                key={item.id}
                variant="outline"
                className="h-auto justify-start py-3 text-left"
                onClick={() => {
                  onOpenChange(false);
                  setSearch("");
                  onSelect(item.id);
                }}
              >
                <Copy data-icon="inline-start" />
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className="truncate font-medium">{item.title}</span>
                  <span className="truncate text-xs text-muted-foreground">
                    {item.description}
                  </span>
                </span>
              </Button>
            ))}
          </div>
        ) : (
          <EmptyState
            title={emptyTitle}
            description={t("Try another search or create one from scratch.")}
            className="min-h-40 border"
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function SetupStepCard({
  number,
  icon: Icon,
  title,
  description,
  countLabel,
  complete,
  actionLabel,
  onAction,
}: {
  number: number;
  icon: LucideIcon;
  title: string;
  description: string;
  countLabel: string;
  complete: boolean;
  actionLabel: string;
  onAction: () => void;
}) {
  const { t } = useI18n();
  return (
    <Card size="sm" data-setup-step={number}>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Badge variant={complete ? "success" : "secondary"}>
            {complete ? <Check aria-hidden="true" /> : number}
          </Badge>
          <Icon aria-hidden="true" />
          {title}
        </CardTitle>
        <CardDescription>{description}</CardDescription>
        <CardAction>
          <Badge variant={complete ? "success" : "warning"}>
            {t(complete ? "Stage ready" : "Needs setup")}
          </Badge>
        </CardAction>
      </CardHeader>
      <CardFooter className="justify-between gap-2">
        <span className="text-xs text-muted-foreground">{countLabel}</span>
        <Button variant="ghost" size="sm" onClick={onAction}>
          {actionLabel}
          <ArrowRight data-icon="inline-end" />
        </Button>
      </CardFooter>
    </Card>
  );
}

function SupplierGroupsPanel({
  groups,
  channels,
  onNewGroup,
  onNewSupplier,
  onCopySupplier,
  onEditGroup,
  onEditChannel,
}: {
  groups: ChannelGroupView[];
  channels: ChannelView[];
  onNewGroup: () => void;
  onNewSupplier: () => void;
  onCopySupplier: (channel: ChannelView) => void;
  onEditGroup: (group: ChannelGroupView) => void;
  onEditChannel: (channel: ChannelView) => void;
}) {
  const { t } = useI18n();
  const channelsByGroup = useMemo(() => {
    const result = new Map<string, ChannelView[]>();
    for (const channel of channels) {
      result.set(channel.channel_group_id, [
        ...(result.get(channel.channel_group_id) ?? []),
        channel,
      ]);
    }
    return result;
  }, [channels]);

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Supplier endpoints and channel groups")}</CardTitle>
        <CardDescription>
          {t(
            "A supplier endpoint is stored as a channel. Channel groups pool endpoints that share one API format.",
          )}
        </CardDescription>
        <CardAction className="col-span-2 col-start-1 row-span-1 row-start-3 mt-2 flex flex-wrap justify-start gap-2 sm:col-span-1 sm:col-start-2 sm:row-span-2 sm:row-start-1 sm:mt-0 sm:justify-end">
          <Button variant="outline" size="sm" onClick={onNewGroup}>
            <Plus data-icon="inline-start" />
            {t("New channel group")}
          </Button>
          <Button size="sm" onClick={onNewSupplier}>
            <Plus data-icon="inline-start" />
            {t("Add supplier")}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        {groups.length > 0 ? (
          <div className="grid gap-4 xl:grid-cols-2">
            {groups.map((group) => {
              const groupedChannels = channelsByGroup.get(group.id) ?? [];
              return (
                <Card key={group.id} size="sm">
                  <CardHeader>
                    <CardTitle>{group.name}</CardTitle>
                    <CardDescription>
                      <span className="flex flex-wrap items-center gap-2">
                        <StatusBadge
                          value={group.api_format}
                          label={apiFormatLabel(group.api_format)}
                          variant="info"
                        />
                        <StatusBadge value={group.enabled} />
                        <Badge variant="secondary">
                          {t("{count} suppliers", {
                            count: groupedChannels.length,
                          })}
                        </Badge>
                      </span>
                    </CardDescription>
                    <CardAction>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("Edit {name}", { name: group.name })}
                        onClick={() => onEditGroup(group)}
                      >
                        <Pencil />
                      </Button>
                    </CardAction>
                  </CardHeader>
                  <CardContent>
                    {groupedChannels.length > 0 ? (
                      <div className="flex flex-col">
                        {groupedChannels.map((channel, index) => (
                          <div key={channel.id}>
                            {index > 0 ? <Separator className="my-3" /> : null}
                            <div className="flex items-center justify-between gap-3">
                              <div className="flex min-w-0 flex-col gap-1">
                                <span className="flex flex-wrap items-center gap-2">
                                  <span className="truncate font-medium">
                                    {channel.name}
                                  </span>
                                  <StatusBadge value={channel.enabled} />
                                </span>
                                <span
                                  className="truncate text-xs text-muted-foreground"
                                  title={channel.base_url}
                                >
                                  {channel.base_url}
                                </span>
                                <span className="text-xs text-muted-foreground">
                                  {t("{count} available models", {
                                    count: channel.available_models.length,
                                  })}
                                </span>
                              </div>
                              <div className="flex shrink-0 items-center gap-1">
                                <Button
                                  variant="ghost"
                                  size="icon-sm"
                                  aria-label={t("Copy {name}", {
                                    name: channel.name,
                                  })}
                                  onClick={() => onCopySupplier(channel)}
                                >
                                  <Copy />
                                </Button>
                                <Button
                                  variant="ghost"
                                  size="icon-sm"
                                  aria-label={t("Edit {name}", {
                                    name: channel.name,
                                  })}
                                  onClick={() => onEditChannel(channel)}
                                >
                                  <Pencil />
                                </Button>
                              </div>
                            </div>
                          </div>
                        ))}
                      </div>
                    ) : (
                      <EmptyState
                        title={t("No supplier endpoints")}
                        description={t(
                          "Add a supplier endpoint to make this channel group usable.",
                        )}
                        className="min-h-32 border"
                      />
                    )}
                  </CardContent>
                </Card>
              );
            })}
          </div>
        ) : (
          <EmptyState
            title={t("No channel groups")}
            description={t(
              "Create a channel group first, then add a supplier endpoint.",
            )}
            className="min-h-48 border"
            actions={
              <Button onClick={onNewGroup}>
                <Plus data-icon="inline-start" />
                {t("New channel group")}
              </Button>
            }
          />
        )}
      </CardContent>
    </Card>
  );
}

export function ModelSetupPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const { t } = useI18n();
  const groups = useChannelGroups();
  const channels = useChannels();
  const models = useModels();
  const rules = useModelRules();
  const [copySupplierOpen, setCopySupplierOpen] = useState(false);
  const [copyModelOpen, setCopyModelOpen] = useState(false);

  const requestedView = searchParams.get("view");
  const view: SetupView = SETUP_VIEWS.includes(requestedView as SetupView)
    ? (requestedView as SetupView)
    : "overview";
  const setupReturnPath = (nextView: SetupView) =>
    nextView === "overview"
      ? MODEL_SETUP_PATH
      : `${MODEL_SETUP_PATH}?view=${nextView}`;
  const setView = (nextView: SetupView) => {
    const next = new URLSearchParams(searchParams);
    if (nextView === "overview") next.delete("view");
    else next.set("view", nextView);
    setSearchParams(next, { replace: true });
  };

  const standardGroups = useMemo(
    () =>
      (groups.data ?? []).filter(
        (group) => group.connector_kind === "openai_compatible",
      ),
    [groups.data],
  );
  const standardGroupIds = useMemo(
    () => new Set(standardGroups.map((group) => group.id)),
    [standardGroups],
  );
  const enabledStandardGroups = useMemo(
    () => standardGroups.filter((group) => group.enabled),
    [standardGroups],
  );
  const enabledStandardGroupIds = useMemo(
    () => new Set(enabledStandardGroups.map((group) => group.id)),
    [enabledStandardGroups],
  );
  const supplierChannels = useMemo(
    () =>
      (channels.data ?? []).filter(
        (channel) =>
          !channel.provider_managed &&
          standardGroupIds.has(channel.channel_group_id),
      ),
    [channels.data, standardGroupIds],
  );
  const readySupplierChannels = useMemo(
    () =>
      supplierChannels.filter(
        (channel) =>
          channel.enabled &&
          !channel.auto_disabled &&
          channel.available_models.length > 0 &&
          (channel.upstream_auth_kind === "none" ||
            channel.upstream_credential_configured) &&
          enabledStandardGroupIds.has(channel.channel_group_id),
      ),
    [enabledStandardGroupIds, supplierChannels],
  );
  const enabledModels = useMemo(
    () => (models.data ?? []).filter((model) => model.enabled),
    [models.data],
  );
  const enabledRules = useMemo(
    () => (rules.data ?? []).filter((rule) => rule.enabled),
    [rules.data],
  );
  const readyRules = useMemo(
    () =>
      enabledRules.filter((rule) => rule.routing_status === "ready"),
    [enabledRules],
  );
  const publishedModelIds = useMemo(
    () => new Set(enabledRules.map((rule) => rule.upstream_model_id)),
    [enabledRules],
  );
  const advertisedModelIds = useMemo(
    () =>
      new Set(
        readySupplierChannels.flatMap((channel) => channel.available_models),
      ),
    [readySupplierChannels],
  );
  const routableModels = enabledModels.filter((model) =>
    advertisedModelIds.has(model.source_model_id),
  );
  const routableModelIds = new Set(
    routableModels.map((model) => model.id),
  );
  const routableModelById = new Map(
    routableModels.map((model) => [model.id, model]),
  );
  const readySetupRules = readyRules.filter((rule) => {
    if (!routableModelIds.has(rule.upstream_model_id)) return false;
    const model = routableModelById.get(rule.upstream_model_id);
    if (!model) return false;
    return readySupplierChannels.some(
      (channel) =>
        channel.available_models.includes(model.source_model_id) &&
        (rule.channel_ids.includes(channel.id) ||
          rule.channel_group_ids.includes(channel.channel_group_id)),
    );
  });
  const unattachedModels = enabledModels.filter(
    (model) => !advertisedModelIds.has(model.source_model_id),
  );
  const unpublishedModels = enabledModels.filter(
    (model) => !publishedModelIds.has(model.id),
  );
  const degradedRules = enabledRules.filter(
    (rule) => rule.routing_status !== "ready",
  );
  const completedSteps = [
    enabledStandardGroups.length > 0,
    readySupplierChannels.length > 0,
    routableModels.length > 0,
    readySetupRules.length > 0,
  ].filter(Boolean).length;
  const progress = completedSteps * 25;

  const modelById = useMemo(
    () => new Map((models.data ?? []).map((model) => [model.id, model])),
    [models.data],
  );
  const groupById = useMemo(
    () => new Map((groups.data ?? []).map((group) => [group.id, group])),
    [groups.data],
  );
  const channelById = useMemo(
    () => new Map((channels.data ?? []).map((channel) => [channel.id, channel])),
    [channels.data],
  );
  const setupLoading =
    groups.isLoading ||
    channels.isLoading ||
    models.isLoading ||
    rules.isLoading;
  const setupError =
    groups.error ?? channels.error ?? models.error ?? rules.error;

  const newGroup = () =>
    navigate(
      adminPath("/admin/routing/channel-groups/new", {
        returnTo: setupReturnPath("suppliers"),
      }),
    );
  const newSupplier = () => {
    if (standardGroups.length === 0) {
      newGroup();
      return;
    }
    const onlyGroup =
      enabledStandardGroups.length === 1 ? enabledStandardGroups[0] : null;
    navigate(
      adminPath("/admin/routing/channels/new", {
        channelGroupId: onlyGroup?.id,
        returnTo: setupReturnPath("suppliers"),
      }),
    );
  };
  const copySupplier = (channel: ChannelView) =>
    navigate(
      adminPath("/admin/routing/channels/new", {
        copyFrom: channel.id,
        channelGroupId: channel.channel_group_id,
        returnTo: setupReturnPath("suppliers"),
      }),
    );
  const newModel = () =>
    navigate(
      adminPath("/admin/models/new", {
        returnTo: setupReturnPath("models"),
      }),
    );
  const copyModel = (model: ControlPlaneModel) =>
    navigate(
      adminPath("/admin/models/new", {
        copyFrom: model.id,
        returnTo: setupReturnPath("models"),
      }),
    );
  const newRule = (model?: ControlPlaneModel) =>
    navigate(
      adminPath("/admin/routing/model-rules/new", {
        upstreamModelId: model?.id,
        clientModel: model?.source_model_id,
        returnTo: setupReturnPath("rules"),
      }),
    );

  const firstIncompleteAction =
    enabledStandardGroups.length === 0
      ? newGroup
      : readySupplierChannels.length === 0
        ? newSupplier
        : routableModels.length === 0
          ? newModel
          : readySetupRules.length === 0
            ? () => newRule()
            : () => setView("rules");
  const continueLabel =
    completedSteps === 4 ? t("Review published routes") : t("Continue setup");

  const modelColumns: Column<ControlPlaneModel>[] = [
    {
      key: "model",
      header: t("Model"),
      render: (model) => (
        <span className="flex flex-col gap-0.5">
          <span className="font-medium">{model.display_name}</span>
          <span className="font-mono text-xs text-muted-foreground">
            {model.source_model_id}
          </span>
        </span>
      ),
    },
    {
      key: "provider",
      header: t("Provider"),
      render: (model) => model.provider_name ?? "—",
    },
    {
      key: "pricing",
      header: t("Input / output price"),
      render: (model) =>
        `${formatDecimal(model.input_unit_price)} / ${formatDecimal(
          model.output_unit_price,
        )}`,
    },
    {
      key: "published",
      header: t("Publication"),
      render: (model) => (
        <Badge variant={publishedModelIds.has(model.id) ? "success" : "warning"}>
          {t(publishedModelIds.has(model.id) ? "Published" : "Not published")}
        </Badge>
      ),
    },
    {
      key: "actions",
      header: t("Actions"),
      className: "text-right",
      render: (model) => (
        <div className="flex justify-end gap-1">
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("Create rule for {model}", {
              model: model.display_name,
            })}
            onClick={() => newRule(model)}
          >
            <Route />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("Configure pricing for {model}", {
              model: model.display_name,
            })}
            onClick={() =>
              navigate(
                adminPath(`/admin/models/${model.id}/pricing`, {
                  returnTo: setupReturnPath("models"),
                }),
              )
            }
          >
            <Calculator />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("Copy {name}", { name: model.display_name })}
            onClick={() => copyModel(model)}
          >
            <Copy />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("Edit {name}", { name: model.display_name })}
            onClick={() =>
              navigate(
                adminPath(`/admin/models/${model.id}`, {
                  returnTo: setupReturnPath("models"),
                }),
              )
            }
          >
            <Pencil />
          </Button>
        </div>
      ),
    },
  ];

  const ruleColumns: Column<ModelRuleView>[] = [
    {
      key: "client_model",
      header: t("Client model"),
      render: (rule) => (
        <span className="flex flex-col gap-1">
          <span className="font-medium">{rule.client_model}</span>
          <StatusBadge
            value={rule.api_format}
            label={apiFormatLabel(rule.api_format)}
            variant="info"
          />
        </span>
      ),
    },
    {
      key: "upstream",
      header: t("Upstream model"),
      render: (rule) =>
        modelById.get(rule.upstream_model_id)?.display_name ??
        rule.upstream_model,
    },
    {
      key: "targets",
      header: t("Route"),
      render: (rule) => (
        <span className="flex flex-wrap gap-1">
          {rule.channel_group_ids.slice(0, 2).map((groupId) => (
            <Badge key={groupId} variant="secondary">
              {groupById.get(groupId)?.name ?? t("Channel group")}
            </Badge>
          ))}
          {rule.channel_ids.length > 0 ? (
            <Badge variant="outline">
              {t("{count} individual channels", {
                count: rule.channel_ids.length,
              })}
            </Badge>
          ) : null}
        </span>
      ),
    },
    {
      key: "status",
      header: t("Routing status"),
      render: (rule) => <StatusBadge value={rule.routing_status} />,
    },
    {
      key: "actions",
      header: t("Actions"),
      className: "text-right",
      render: (rule) => (
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("Edit {name}", { name: rule.client_model })}
          onClick={() =>
            navigate(
              adminPath(`/admin/routing/model-rules/${rule.id}`, {
                returnTo: setupReturnPath("rules"),
              }),
            )
          }
        >
          <Pencil />
        </Button>
      ),
    },
  ];

  const supplierPickerItems: PickerItem[] = supplierChannels.map((channel) => ({
    id: channel.id,
    title: channel.name,
    description: `${
      groupById.get(channel.channel_group_id)?.name ?? t("Channel group")
    } · ${channel.base_url}`,
    searchText: `${channel.name} ${channel.base_url} ${
      groupById.get(channel.channel_group_id)?.name ?? ""
    }`,
  }));
  const modelPickerItems: PickerItem[] = (models.data ?? []).map((model) => ({
    id: model.id,
    title: model.display_name,
    description: `${model.provider_name ?? t("Unspecified provider")} · ${
      model.source_model_id
    }`,
    searchText: `${model.display_name} ${model.provider_name ?? ""} ${
      model.source_model_id
    }`,
  }));

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Model setup")}
        description={t(
          "Connect a supplier, price its models, and publish client-facing routes from one guided workspace.",
        )}
        actions={
          <>
            <Button
              variant="outline"
              onClick={() => navigate("/admin/catalog")}
            >
              <Calculator data-icon="inline-start" />
              {t("Import prices")}
            </Button>
            <Button
              onClick={firstIncompleteAction}
              disabled={setupLoading || Boolean(setupError)}
            >
              <Sparkles data-icon="inline-start" />
              {continueLabel}
            </Button>
          </>
        }
      />

      <AsyncResource
        isLoading={setupLoading}
        error={setupError}
      >
        <Card>
          <CardHeader>
            <CardTitle>{t("One flow from endpoint to client model")}</CardTitle>
            <CardDescription>
              {t(
                "Complete the four stages in order. Existing configuration is reused, so you can resume at any stage.",
              )}
            </CardDescription>
            <CardAction>
              <Badge variant={completedSteps === 4 ? "success" : "info"}>
                {t("{complete} of {total} ready", {
                  complete: completedSteps,
                  total: 4,
                })}
              </Badge>
            </CardAction>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <Progress
              value={progress}
              aria-label={t("Model setup progress")}
            />
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
              <SetupStepCard
                number={1}
                icon={Layers3}
                title={t("Channel group")}
                description={t(
                  "Choose the API format and the pool that will own supplier endpoints.",
                )}
                countLabel={t("{count} enabled groups", {
                  count: enabledStandardGroups.length,
                })}
                complete={enabledStandardGroups.length > 0}
                actionLabel={t("Manage")}
                onAction={() => setView("suppliers")}
              />
              <SetupStepCard
                number={2}
                icon={Network}
                title={t("Supplier endpoint")}
                description={t(
                  "Configure the base URL, credential, capabilities, and available model IDs.",
                )}
                countLabel={t("{count} ready suppliers", {
                  count: readySupplierChannels.length,
                })}
                complete={readySupplierChannels.length > 0}
                actionLabel={t("Add or copy")}
                onAction={() => setView("suppliers")}
              />
              <SetupStepCard
                number={3}
                icon={Boxes}
                title={t("Model and pricing")}
                description={t(
                  "Register upstream model IDs and the prices used for settlement.",
                )}
                countLabel={t("{count} routable models", {
                  count: routableModels.length,
                })}
                complete={routableModels.length > 0}
                actionLabel={t("Add or copy")}
                onAction={() => setView("models")}
              />
              <SetupStepCard
                number={4}
                icon={Route}
                title={t("Model rule")}
                description={t(
                  "Publish a client model name and connect it to compatible routing targets.",
                )}
                countLabel={t("{count} ready rules", {
                  count: readySetupRules.length,
                })}
                complete={readySetupRules.length > 0}
                actionLabel={t("Publish")}
                onAction={() => setView("rules")}
              />
            </div>
          </CardContent>
        </Card>

        <Tabs
          value={view}
          onValueChange={(value) => setView(value as SetupView)}
        >
          <TabsList
            variant="line"
            className="max-w-full justify-start overflow-x-auto overflow-y-hidden"
          >
            <TabsTrigger value="overview">
              <Workflow aria-hidden="true" />
              {t("Overview")}
            </TabsTrigger>
            <TabsTrigger value="suppliers">
              <Network aria-hidden="true" />
              {t("Groups and suppliers")}
            </TabsTrigger>
            <TabsTrigger value="models">
              <Boxes aria-hidden="true" />
              {t("Models and pricing")}
            </TabsTrigger>
            <TabsTrigger value="rules">
              <Route aria-hidden="true" />
              {t("Published routes")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="overview" className="flex flex-col gap-6 pt-4">
            <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,1.35fr)_minmax(20rem,0.65fr)]">
              <Card>
                <CardHeader>
                  <CardTitle>{t("Published routing map")}</CardTitle>
                  <CardDescription>
                    {t(
                      "Review the complete path from the client model name to its upstream model and routing targets.",
                    )}
                  </CardDescription>
                  <CardAction>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setView("rules")}
                    >
                      {t("View all")}
                      <ArrowRight data-icon="inline-end" />
                    </Button>
                  </CardAction>
                </CardHeader>
                <CardContent>
                  {enabledRules.length > 0 ? (
                    <div className="flex flex-col">
                      {enabledRules.slice(0, 5).map((rule, index) => {
                        const model = modelById.get(rule.upstream_model_id);
                        const targetNames = [
                          ...rule.channel_group_ids.map(
                            (groupId) =>
                              groupById.get(groupId)?.name ??
                              t("Channel group"),
                          ),
                          ...rule.channel_ids.map(
                            (channelId) =>
                              channelById.get(channelId)?.name ??
                              t("Individual channel"),
                          ),
                        ];
                        return (
                          <div key={rule.id}>
                            {index > 0 ? (
                              <Separator className="my-4" />
                            ) : null}
                            <div className="grid items-center gap-3 md:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_minmax(0,1fr)]">
                              <span className="flex min-w-0 flex-col gap-1">
                                <span className="text-xs text-muted-foreground md:sr-only">
                                  {t("Client model")}
                                </span>
                                <span className="truncate font-medium">
                                  {rule.client_model}
                                </span>
                                <StatusBadge
                                  value={rule.api_format}
                                  label={apiFormatLabel(rule.api_format)}
                                  variant="info"
                                />
                              </span>
                              <ArrowRight
                                aria-hidden="true"
                                className="hidden md:block"
                              />
                              <span className="flex min-w-0 flex-col gap-1">
                                <span className="text-xs text-muted-foreground md:sr-only">
                                  {t("Upstream model")}
                                </span>
                                <span className="truncate font-medium">
                                  {model?.display_name ?? rule.upstream_model}
                                </span>
                                <span className="truncate font-mono text-xs text-muted-foreground">
                                  {model?.source_model_id ??
                                    rule.upstream_model}
                                </span>
                              </span>
                              <ArrowRight
                                aria-hidden="true"
                                className="hidden md:block"
                              />
                              <span className="flex min-w-0 flex-col gap-1">
                                <span className="text-xs text-muted-foreground md:sr-only">
                                  {t("Targets")}
                                </span>
                                <span className="flex flex-wrap gap-1">
                                  {targetNames.slice(0, 2).map((target, targetIndex) => (
                                    <Badge
                                      key={`${target}-${targetIndex}`}
                                      variant="secondary"
                                    >
                                      {target}
                                    </Badge>
                                  ))}
                                  {targetNames.length > 2 ? (
                                    <Badge variant="outline">
                                      +{targetNames.length - 2}
                                    </Badge>
                                  ) : null}
                                </span>
                                <StatusBadge value={rule.routing_status} />
                              </span>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  ) : (
                    <EmptyState
                      title={t("No published model routes")}
                      description={t(
                        "Create a model rule after adding a supplier and a priced model.",
                      )}
                      className="min-h-48 border"
                      actions={
                        <Button onClick={() => newRule()}>
                          <Plus data-icon="inline-start" />
                          {t("Add model rule")}
                        </Button>
                      }
                    />
                  )}
                </CardContent>
              </Card>

              <div className="flex flex-col gap-6">
                <Card>
                  <CardHeader>
                    <CardTitle>{t("Quick actions")}</CardTitle>
                    <CardDescription>
                      {t("Start from scratch or reuse a proven configuration.")}
                    </CardDescription>
                  </CardHeader>
                  <CardContent className="grid gap-2 sm:grid-cols-2 xl:grid-cols-1">
                    <Button variant="outline" onClick={newSupplier}>
                      <Plus data-icon="inline-start" />
                      {t("Add supplier")}
                    </Button>
                    <Button
                      variant="outline"
                      disabled={supplierChannels.length === 0}
                      onClick={() => setCopySupplierOpen(true)}
                    >
                      <Copy data-icon="inline-start" />
                      {t("Copy supplier")}
                    </Button>
                    <Button variant="outline" onClick={newModel}>
                      <Plus data-icon="inline-start" />
                      {t("Add model")}
                    </Button>
                    <Button
                      variant="outline"
                      disabled={(models.data?.length ?? 0) === 0}
                      onClick={() => setCopyModelOpen(true)}
                    >
                      <Copy data-icon="inline-start" />
                      {t("Copy model")}
                    </Button>
                    <Button onClick={() => newRule()}>
                      <Route data-icon="inline-start" />
                      {t("Add model rule")}
                    </Button>
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader>
                    <CardTitle>{t("Configuration health")}</CardTitle>
                    <CardDescription>
                      {t("Resolve these gaps before sending production traffic.")}
                    </CardDescription>
                  </CardHeader>
                  <CardContent className="flex flex-col gap-3">
                    <div className="flex items-center justify-between gap-3">
                      <span className="flex items-center gap-2">
                        {unattachedModels.length === 0 ? (
                          <Check aria-hidden="true" />
                        ) : (
                          <CircleAlert aria-hidden="true" />
                        )}
                        {t("Models not advertised by suppliers")}
                      </span>
                      <Badge
                        variant={
                          unattachedModels.length === 0 ? "success" : "warning"
                        }
                      >
                        {unattachedModels.length}
                      </Badge>
                    </div>
                    <div className="flex items-center justify-between gap-3">
                      <span className="flex items-center gap-2">
                        {unpublishedModels.length === 0 ? (
                          <Check aria-hidden="true" />
                        ) : (
                          <CircleAlert aria-hidden="true" />
                        )}
                        {t("Models not published")}
                      </span>
                      <Badge
                        variant={
                          unpublishedModels.length === 0 ? "success" : "warning"
                        }
                      >
                        {unpublishedModels.length}
                      </Badge>
                    </div>
                    <div className="flex items-center justify-between gap-3">
                      <span className="flex items-center gap-2">
                        {degradedRules.length === 0 ? (
                          <Check aria-hidden="true" />
                        ) : (
                          <CircleAlert aria-hidden="true" />
                        )}
                        {t("Degraded routes")}
                      </span>
                      <Badge
                        variant={
                          degradedRules.length === 0 ? "success" : "warning"
                        }
                      >
                        {degradedRules.length}
                      </Badge>
                    </div>
                  </CardContent>
                </Card>
              </div>
            </div>
          </TabsContent>

          <TabsContent value="suppliers" className="pt-4">
            <SupplierGroupsPanel
              groups={standardGroups}
              channels={supplierChannels}
              onNewGroup={newGroup}
              onNewSupplier={newSupplier}
              onCopySupplier={copySupplier}
              onEditGroup={(group) =>
                navigate(
                  adminPath(`/admin/routing/channel-groups/${group.id}`, {
                    returnTo: setupReturnPath("suppliers"),
                  }),
                )
              }
              onEditChannel={(channel) =>
                navigate(
                  adminPath(`/admin/routing/channels/${channel.id}`, {
                    returnTo: setupReturnPath("suppliers"),
                  }),
                )
              }
            />
          </TabsContent>

          <TabsContent value="models" className="pt-4">
            <Card>
              <CardHeader>
                <CardTitle>{t("Models and pricing")}</CardTitle>
                <CardDescription>
                  {t(
                    "Register each upstream model once, then publish it through one or more API formats.",
                  )}
                </CardDescription>
                <CardAction className="col-span-2 col-start-1 row-span-1 row-start-3 mt-2 flex flex-wrap justify-start gap-2 sm:col-span-1 sm:col-start-2 sm:row-span-2 sm:row-start-1 sm:mt-0 sm:justify-end">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={(models.data?.length ?? 0) === 0}
                    onClick={() => setCopyModelOpen(true)}
                  >
                    <Copy data-icon="inline-start" />
                    {t("Copy model")}
                  </Button>
                  <Button size="sm" onClick={newModel}>
                    <Plus data-icon="inline-start" />
                    {t("Add model")}
                  </Button>
                </CardAction>
              </CardHeader>
              <CardContent>
                <ResourceTable
                  columns={modelColumns}
                  rows={models.data ?? []}
                  rowKey={(model) => model.id}
                  onRowClick={(model) =>
                    navigate(
                      adminPath(`/admin/models/${model.id}`, {
                        returnTo: setupReturnPath("models"),
                      }),
                    )
                  }
                  empty={
                    <EmptyState
                      title={t("No upstream models")}
                      description={t(
                        "Add a model and its prices before creating a model rule.",
                      )}
                      className="min-h-48 border"
                    />
                  }
                />
              </CardContent>
            </Card>
          </TabsContent>

          <TabsContent value="rules" className="pt-4">
            <Card>
              <CardHeader>
                <CardTitle>{t("Published routes")}</CardTitle>
                <CardDescription>
                  {t(
                    "Each rule exposes one client model name for one API format and sends it to compatible targets.",
                  )}
                </CardDescription>
                <CardAction>
                  <Button size="sm" onClick={() => newRule()}>
                    <Plus data-icon="inline-start" />
                    {t("Add model rule")}
                  </Button>
                </CardAction>
              </CardHeader>
              <CardContent>
                <ResourceTable
                  columns={ruleColumns}
                  rows={rules.data ?? []}
                  rowKey={(rule) => rule.id}
                  onRowClick={(rule) =>
                    navigate(
                      adminPath(`/admin/routing/model-rules/${rule.id}`, {
                        returnTo: setupReturnPath("rules"),
                      }),
                    )
                  }
                  empty={
                    <EmptyState
                      title={t("No published model routes")}
                      description={t(
                        "Add a rule to expose a priced upstream model to clients.",
                      )}
                      className="min-h-48 border"
                    />
                  }
                />
              </CardContent>
            </Card>
          </TabsContent>
        </Tabs>
      </AsyncResource>

      <CopyPickerDialog
        open={copySupplierOpen}
        title={t("Copy supplier")}
        description={t(
          "Choose a supplier endpoint to reuse its connection, routing, and model-capability settings. Credentials are not copied.",
        )}
        emptyTitle={t("No supplier endpoints")}
        items={supplierPickerItems}
        onOpenChange={setCopySupplierOpen}
        onSelect={(id) => {
          const channel = supplierChannels.find((item) => item.id === id);
          if (channel) copySupplier(channel);
        }}
      />
      <CopyPickerDialog
        open={copyModelOpen}
        title={t("Copy model")}
        description={t(
          "Choose a model to reuse its provider and pricing settings. Enter a new source model ID before saving.",
        )}
        emptyTitle={t("No upstream models")}
        items={modelPickerItems}
        onOpenChange={setCopyModelOpen}
        onSelect={(id) => {
          const model = models.data?.find((item) => item.id === id);
          if (model) copyModel(model);
        }}
      />
    </div>
  );
}
