import { type ReactNode, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import {
  CheckCheck,
  ChevronDown,
  ListChecks,
  Pencil,
  Plus,
  Power,
  PowerOff,
  RotateCcw,
  Search,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import {
  ToggleGroup,
  ToggleGroupItem,
} from "@/components/ui/toggle-group";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import {
  useBatchUpdateChannels,
  useChannelGroups,
  useChannels,
  useRecoverChannel,
  useSetChannelGroupEnabled,
} from "@/features/admin/api";
import { ChannelBatchEditDialog } from "@/features/admin/routing/channels/channel-batch-edit-dialog";
import { formatRelative } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";
import {
  apiFormatLabel,
  connectorKindLabel,
  selectionStrategyLabel,
} from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";
import type { ChannelGroupView, ChannelView } from "@/api/types";

type GroupFilter = "all" | "standard" | "codex";

interface StandardGroupEntry {
  group: ChannelGroupView;
  channels: ChannelView[];
}

interface CodexPoolEntry {
  id: string;
  canonicalGroup: ChannelGroupView;
  groups: ChannelGroupView[];
  credentialCount: number;
}

const FORMAT_ORDER: Record<ChannelGroupView["api_format"], number> = {
  open_ai_chat_completions: 0,
  open_ai_responses: 1,
  open_ai_images: 2,
};

function compareGroups(left: ChannelGroupView, right: ChannelGroupView): number {
  return (
    left.priority - right.priority ||
    left.name.localeCompare(right.name) ||
    FORMAT_ORDER[left.api_format] - FORMAT_ORDER[right.api_format]
  );
}

function matchesSearch(value: string, search: string): boolean {
  return value.toLowerCase().includes(search);
}

function StandardGroupCard({
  group,
  channels,
  preferOpen,
  children,
  onEdit,
  onDisable,
  disablePending,
}: {
  group: ChannelGroupView;
  channels: ChannelView[];
  preferOpen: boolean;
  children: ReactNode;
  onEdit: () => void;
  onDisable: () => void;
  disablePending: boolean;
}) {
  const { t } = useI18n();
  const [open, setOpen] = useState(preferOpen);
  const titleId = `channel-group-${group.id}`;

  useEffect(() => {
    if (preferOpen) setOpen(true);
  }, [preferOpen]);

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <Card size="sm" role="region" aria-labelledby={titleId}>
        <CardHeader>
          <CardTitle id={titleId}>{group.name}</CardTitle>
          <CardDescription>
            <span className="flex flex-wrap items-center gap-2">
              <StatusBadge
                value={group.api_format}
                label={apiFormatLabel(group.api_format)}
                variant="info"
              />
              <StatusBadge value={group.enabled} />
              <Badge variant="secondary">
                {t("Channels ({count})", { count: channels.length })}
              </Badge>
              <Badge variant="outline">
                {t("Priority")}: {group.priority}
              </Badge>
              <Badge variant="outline">
                {selectionStrategyLabel(group.selection_strategy)}
              </Badge>
            </span>
          </CardDescription>
          <CardAction className="flex flex-wrap items-center justify-end gap-1">
            {group.enabled ? (
              <Button
                variant="ghost"
                size="sm"
                disabled={disablePending}
                onClick={onDisable}
              >
                <PowerOff data-icon="inline-start" />
                {t("Disable group")}
              </Button>
            ) : null}
            <Button variant="ghost" size="sm" onClick={onEdit}>
              <Pencil data-icon="inline-start" />
              {t("Edit group")}
            </Button>
            <CollapsibleTrigger
              render={<Button variant="outline" size="sm" />}
            >
              {open
                ? t("Hide channels")
                : t("Show channels ({count})", { count: channels.length })}
              <ChevronDown
                data-icon="inline-end"
                className={cn("transition-transform", open && "rotate-180")}
              />
            </CollapsibleTrigger>
          </CardAction>
        </CardHeader>
        <CollapsibleContent>
          <CardContent>{children}</CardContent>
        </CollapsibleContent>
      </Card>
    </Collapsible>
  );
}

export function ChannelsPage() {
  const navigate = useNavigate();
  const groups = useChannelGroups();
  const channels = useChannels();
  const { t } = useI18n();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [batchOpen, setBatchOpen] = useState(false);
  const [disableTarget, setDisableTarget] = useState<ChannelView | null>(null);
  const [disableGroupTarget, setDisableGroupTarget] =
    useState<ChannelGroupView | null>(null);
  const [search, setSearch] = useState("");
  const [groupFilter, setGroupFilter] = useState<GroupFilter>("all");
  const quickUpdate = useBatchUpdateChannels();
  const setGroupEnabled = useSetChannelGroupEnabled();
  const recoverChannel = useRecoverChannel();
  const batchDialogTriggerId = "channel-batch-edit-trigger";
  const normalizedSearch = search.trim().toLowerCase();
  const ordinaryChannels = useMemo(
    () => (channels.data ?? []).filter((channel) => !channel.provider_managed),
    [channels.data],
  );

  useEffect(() => {
    const available = new Set(ordinaryChannels.map((channel) => channel.id));
    setSelected((current) => {
      const next = new Set([...current].filter((id) => available.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [ordinaryChannels]);

  const channelsByGroup = useMemo(() => {
    const grouped = new Map<string, ChannelView[]>();
    for (const channel of channels.data ?? []) {
      const current = grouped.get(channel.channel_group_id) ?? [];
      grouped.set(channel.channel_group_id, [...current, channel]);
    }
    return grouped;
  }, [channels.data]);
  const groupById = useMemo(
    () => new Map((groups.data ?? []).map((group) => [group.id, group])),
    [groups.data],
  );

  const standardGroupCount = (groups.data ?? []).filter(
    (group) => group.connector_kind === "openai_compatible",
  ).length;
  const codexPoolCount = new Set(
    (groups.data ?? [])
      .filter((group) => group.connector_kind === "codex_oauth")
      .map((group) => group.connector_pool_id ?? group.id),
  ).size;

  const standardEntries = useMemo<StandardGroupEntry[]>(() => {
    if (groupFilter === "codex") return [];
    return (groups.data ?? [])
      .filter((group) => group.connector_kind === "openai_compatible")
      .sort(compareGroups)
      .flatMap((group) => {
        const groupedChannels = (channelsByGroup.get(group.id) ?? []).filter(
          (channel) => !channel.provider_managed,
        );
        if (!normalizedSearch) {
          return [{ group, channels: groupedChannels }];
        }
        const groupMatches =
          matchesSearch(group.name, normalizedSearch) ||
          matchesSearch(apiFormatLabel(group.api_format), normalizedSearch);
        const matchingChannels = groupedChannels.filter(
          (channel) =>
            matchesSearch(channel.name, normalizedSearch) ||
            matchesSearch(channel.base_url, normalizedSearch),
        );
        if (!groupMatches && matchingChannels.length === 0) return [];
        return [
          {
            group,
            channels: groupMatches ? groupedChannels : matchingChannels,
          },
        ];
      });
  }, [channelsByGroup, groupFilter, groups.data, normalizedSearch]);

  const codexPoolEntries = useMemo<CodexPoolEntry[]>(() => {
    if (groupFilter === "standard") return [];
    const pools = new Map<string, ChannelGroupView[]>();
    for (const group of groups.data ?? []) {
      if (group.connector_kind !== "codex_oauth") continue;
      const poolId = group.connector_pool_id ?? group.id;
      pools.set(poolId, [...(pools.get(poolId) ?? []), group]);
    }

    return [...pools.entries()]
      .flatMap(([id, poolGroups]) => {
        const sortedGroups = [...poolGroups].sort(
          (left, right) =>
            FORMAT_ORDER[left.api_format] - FORMAT_ORDER[right.api_format],
        );
        const canonicalGroup =
          sortedGroups.find((group) => group.api_format === "open_ai_responses") ??
          sortedGroups[0];
        const poolChannels = sortedGroups.flatMap(
          (group) =>
            (channelsByGroup.get(group.id) ?? []).filter(
              (channel) => channel.provider_managed,
            ),
        );
        const poolMatches =
          !normalizedSearch ||
          sortedGroups.some(
            (group) =>
              matchesSearch(group.name, normalizedSearch) ||
              matchesSearch(apiFormatLabel(group.api_format), normalizedSearch),
          ) ||
          poolChannels.some(
            (channel) =>
              matchesSearch(channel.name, normalizedSearch) ||
              matchesSearch(channel.base_url, normalizedSearch),
          );
        if (!poolMatches) return [];
        const responsesGroup = sortedGroups.find(
          (group) => group.api_format === "open_ai_responses",
        );
        const credentialCount = responsesGroup
          ? (channelsByGroup.get(responsesGroup.id) ?? []).filter(
              (channel) => channel.provider_managed,
            ).length
          : Math.max(
              0,
              ...sortedGroups.map(
                (group) =>
                  (channelsByGroup.get(group.id) ?? []).filter(
                    (channel) => channel.provider_managed,
                  ).length,
              ),
            );
        return [{ id, canonicalGroup, groups: sortedGroups, credentialCount }];
      })
      .sort((left, right) =>
        left.canonicalGroup.name.localeCompare(right.canonicalGroup.name),
      );
  }, [channelsByGroup, groupFilter, groups.data, normalizedSearch]);

  const knownGroupIds = useMemo(
    () => new Set((groups.data ?? []).map((group) => group.id)),
    [groups.data],
  );
  const unavailableGroupChannels = useMemo(() => {
    if (groupFilter === "codex") return [];
    return (channels.data ?? []).filter(
      (channel) =>
        !channel.provider_managed &&
        !knownGroupIds.has(channel.channel_group_id) &&
        (!normalizedSearch ||
          matchesSearch(channel.name, normalizedSearch) ||
          matchesSearch(channel.base_url, normalizedSearch)),
    );
  }, [channels.data, groupFilter, knownGroupIds, normalizedSearch]);
  const visibleOrdinaryChannels = useMemo(
    () =>
      [
        ...new Map(
          [
            ...standardEntries.flatMap((entry) => entry.channels),
            ...unavailableGroupChannels,
          ].map((channel) => [channel.id, channel]),
        ).values(),
      ],
    [standardEntries, unavailableGroupChannels],
  );
  const selectedChannels = useMemo(
    () => ordinaryChannels.filter((channel) => selected.has(channel.id)),
    [ordinaryChannels, selected],
  );
  const allSelected =
    visibleOrdinaryChannels.length > 0 &&
    visibleOrdinaryChannels.every((channel) => selected.has(channel.id));

  const toggleChannel = (channel: ChannelView) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(channel.id)) next.delete(channel.id);
      else next.add(channel.id);
      return next;
    });
  };

  const setChannelEnabled = async (channel: ChannelView, enabled: boolean) => {
    try {
      await quickUpdate.mutateAsync({
        items: [{ id: channel.id, updated_at: channel.updated_at }],
        changes: { enabled },
      });
      toast.success(
        t(enabled ? "Enabled {name}." : "Disabled {name}.", {
          name: channel.name,
        }),
      );
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Request failed"));
    }
  };

  const recover = async (channel: ChannelView) => {
    try {
      await recoverChannel.mutateAsync({
        id: channel.id,
        input: { updated_at: channel.updated_at },
      });
      toast.success(t("Recovered {name}.", { name: channel.name }));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Request failed"));
    }
  };

  const disableGroup = async (group: ChannelGroupView) => {
    try {
      await setGroupEnabled.mutateAsync({ group, enabled: false });
      toast.success(
        t("Disabled {name}; all {count} channels in the group are unavailable.", {
          name: group.name,
          count: channelsByGroup.get(group.id)?.length ?? 0,
        }),
      );
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This group was changed elsewhere. Reloading."));
      } else {
        toast.error(
          controlPlaneMutationErrorMessage(error, t("Request failed")),
        );
      }
    }
  };

  const quickActionPending =
    quickUpdate.isPending || recoverChannel.isPending || setGroupEnabled.isPending;

  const columns: Column<ChannelView>[] = [
    {
      key: "selected",
      header: t("Select"),
      render: (channel) => (
        <Checkbox
          aria-label={`${t("Select")} ${channel.name}`}
          checked={selected.has(channel.id)}
          onCheckedChange={() => toggleChannel(channel)}
        />
      ),
      className: "w-12",
    },
    {
      key: "name",
      header: t("Name"),
      render: (channel) => <span className="font-medium">{channel.name}</span>,
    },
    {
      key: "state",
      header: t("State"),
      render: (channel) => {
        const groupDisabled =
          groupById.get(channel.channel_group_id)?.enabled === false;
        return (
          <div className="flex items-center gap-1">
            <StatusBadge value={channel.enabled && !groupDisabled} />
            {groupDisabled ? (
              <Badge variant="warning">{t("group disabled")}</Badge>
            ) : null}
            {channel.auto_disabled ? (
              <Badge variant="destructive">{t("auto-disabled")}</Badge>
            ) : null}
          </div>
        );
      },
    },
    {
      key: "statistics",
      header: t("Status statistics"),
      render: (channel) => <StatusBadge value={channel.status_statistics_enabled} />,
    },
    {
      key: "websocket",
      header: t("WebSocket"),
      render: (channel) => <StatusBadge value={channel.supports_websocket} />,
    },
    { key: "weight", header: t("Weight"), render: (channel) => channel.weight },
    {
      key: "billing_multiplier",
      header: t("Billing multiplier"),
      render: (channel) => formatDecimal(channel.billing_multiplier),
    },
    {
      key: "updated",
      header: t("Updated"),
      render: (channel) => formatRelative(channel.updated_at),
    },
    {
      key: "actions",
      header: t("Actions"),
      render: (channel) => (
        <div className="flex justify-end gap-1">
          {channel.enabled ? (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("Disable {name}", { name: channel.name })}
              disabled={quickActionPending}
              onClick={() => setDisableTarget(channel)}
            >
              <PowerOff />
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("Enable {name}", { name: channel.name })}
              disabled={quickActionPending}
              onClick={() => void setChannelEnabled(channel, true)}
            >
              <Power />
            </Button>
          )}
          {channel.auto_disabled ? (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("Recover {name}", { name: channel.name })}
              disabled={quickActionPending}
              onClick={() => void recover(channel)}
            >
              <RotateCcw />
            </Button>
          ) : null}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("Edit {name}", { name: channel.name })}
            onClick={() => navigate(`/admin/routing/channels/${channel.id}`)}
          >
            <Pencil />
          </Button>
        </div>
      ),
      className: "text-right",
    },
  ];

  const channelTable = (rows: ChannelView[]) => (
    <ResourceTable
      columns={columns}
      rows={rows}
      rowKey={(channel) => channel.id}
      onRowClick={(channel) => navigate(`/admin/routing/channels/${channel.id}`)}
      empty={
        <EmptyState
          title={t("No channels in this group")}
          description={t("Create a channel and assign it to this group.")}
          className="min-h-32 border"
        />
      }
    />
  );

  const hasFilteredResults =
    codexPoolEntries.length > 0 ||
    standardEntries.length > 0 ||
    unavailableGroupChannels.length > 0;
  const preferExpandedStandardGroups =
    normalizedSearch.length > 0 || standardEntries.length <= 3;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Channels")}
        description={t(
          "Browse compact channel groups, with shared Codex credentials kept together.",
        )}
        actions={
          <>
            <Button
              variant="outline"
              disabled={visibleOrdinaryChannels.length === 0}
              onClick={() => {
                if (allSelected) {
                  setSelected(new Set());
                  return;
                }
                setSelected((current) => {
                  const next = new Set(current);
                  for (const channel of visibleOrdinaryChannels) {
                    next.add(channel.id);
                  }
                  return next;
                });
              }}
            >
              <CheckCheck data-icon="inline-start" />
              {allSelected ? t("Clear selection") : t("Select all")}
            </Button>
            <Button
              id={batchDialogTriggerId}
              variant="outline"
              disabled={selectedChannels.length === 0}
              onClick={() => setBatchOpen(true)}
            >
              <ListChecks data-icon="inline-start" />
              {t("Batch edit ({count})", { count: selectedChannels.length })}
            </Button>
            <Button
              variant="outline"
              onClick={() => navigate("/admin/routing/channel-groups/new")}
            >
              <Plus data-icon="inline-start" /> {t("New group")}
            </Button>
            <Button onClick={() => navigate("/admin/routing/channels/new")}>
              <Plus data-icon="inline-start" /> {t("New channel")}
            </Button>
          </>
        }
      />

      <Card size="sm">
        <CardHeader>
          <CardTitle>{t("Channel group directory")}</CardTitle>
          <CardDescription>
            <span className="flex flex-wrap items-center justify-between gap-2">
              <span>
                {t("Search first, then expand only the channel groups you need.")}
              </span>
              <span className="flex flex-wrap gap-2">
                <Badge variant="secondary">
                  {t("Standard groups")}: {standardGroupCount}
                </Badge>
                <Badge variant="secondary">
                  {t("Codex pools")}: {codexPoolCount}
                </Badge>
              </span>
            </span>
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="grid gap-3 xl:grid-cols-[minmax(18rem,1fr)_auto] xl:items-center">
            <InputGroup>
              <InputGroupAddon>
                <Search aria-hidden="true" />
              </InputGroupAddon>
              <InputGroupInput
                type="search"
                value={search}
                aria-label={t("Search groups or channels")}
                placeholder={t("Search groups or channels")}
                onChange={(event) => setSearch(event.target.value)}
              />
              {search ? (
                <InputGroupAddon align="inline-end">
                  <InputGroupButton
                    size="icon-xs"
                    aria-label={t("Clear search")}
                    onClick={() => setSearch("")}
                  >
                    <X data-icon="inline-start" />
                  </InputGroupButton>
                </InputGroupAddon>
              ) : null}
            </InputGroup>
            <ToggleGroup
              value={[groupFilter]}
              onValueChange={(values) => {
                const value = values[0] as GroupFilter | undefined;
                if (value) setGroupFilter(value);
              }}
              variant="outline"
              size="sm"
              spacing={0}
              className="max-w-full overflow-x-auto"
              aria-label={t("Channel group type")}
            >
              <ToggleGroupItem value="all">{t("All groups")}</ToggleGroupItem>
              <ToggleGroupItem value="standard">
                {connectorKindLabel("openai_compatible")}
              </ToggleGroupItem>
              <ToggleGroupItem value="codex">
                {connectorKindLabel("codex_oauth")}
              </ToggleGroupItem>
            </ToggleGroup>
          </div>
        </CardContent>
      </Card>

      <AsyncResource
        isLoading={groups.isLoading || channels.isLoading}
        error={groups.error ?? channels.error}
        isEmpty={(groups.data?.length ?? 0) === 0 && (channels.data?.length ?? 0) === 0}
        emptyTitle={t("No records")}
        emptyDescription={t("There are no records to show yet.")}
      >
        {hasFilteredResults ? (
          <div className="flex flex-col gap-8">
            {codexPoolEntries.length > 0 ? (
              <section
                aria-labelledby="codex-credential-pools"
                className="flex flex-col gap-3"
              >
                <div className="flex flex-wrap items-end justify-between gap-2">
                  <div className="flex flex-col gap-1">
                    <h2
                      id="codex-credential-pools"
                      className="text-lg font-semibold tracking-tight"
                    >
                      {t("Codex credential pools")}
                    </h2>
                    <p className="text-sm text-muted-foreground">
                      {t(
                        "Responses and Images groups derived from the same credential pool are managed together.",
                      )}
                    </p>
                  </div>
                  <Badge variant="secondary">{codexPoolEntries.length}</Badge>
                </div>

                <div className="grid gap-4 xl:grid-cols-2">
                  {codexPoolEntries.map((pool) => {
                    const titleId = `codex-pool-${pool.id}`;
                    const credentialGroup =
                      pool.groups.find(
                        (group) => group.api_format === "open_ai_responses",
                      ) ?? pool.canonicalGroup;
                    return (
                      <Card
                        key={pool.id}
                        role="region"
                        aria-labelledby={titleId}
                      >
                        <CardHeader>
                          <CardTitle id={titleId}>
                            {pool.canonicalGroup.name}
                          </CardTitle>
                          <CardDescription>
                            <span className="flex flex-wrap items-center gap-2">
                              <Badge variant="secondary">
                                {t("Shared credentials")}
                              </Badge>
                              <Badge variant="outline">
                                {t("Credentials ({count})", {
                                  count: pool.credentialCount,
                                })}
                              </Badge>
                            </span>
                          </CardDescription>
                          <CardAction>
                            <Button
                              size="sm"
                              onClick={() =>
                                navigate(
                                  `/admin/providers/codex-oauth/${credentialGroup.id}`,
                                )
                              }
                            >
                              {t("Manage shared credentials")}
                            </Button>
                          </CardAction>
                        </CardHeader>
                        <CardContent>
                          <div className="grid gap-3 md:grid-cols-2">
                            {pool.groups.map((group) => {
                              const groupedChannels = (
                                channelsByGroup.get(group.id) ?? []
                              ).filter((channel) => channel.provider_managed);
                              return (
                                <Card key={group.id} size="sm">
                                  <CardHeader>
                                    <CardTitle>
                                      {apiFormatLabel(group.api_format)}
                                    </CardTitle>
                                    <CardDescription
                                      className="truncate"
                                      title={group.name}
                                    >
                                      {group.name}
                                    </CardDescription>
                                    <CardAction>
                                      <div className="flex items-center gap-1">
                                        {group.enabled ? (
                                          <Button
                                            variant="ghost"
                                            size="icon-sm"
                                            aria-label={t("Disable group {name}", {
                                              name: group.name,
                                            })}
                                            disabled={quickActionPending}
                                            onClick={() => setDisableGroupTarget(group)}
                                          >
                                            <PowerOff />
                                          </Button>
                                        ) : null}
                                        <Button
                                          variant="ghost"
                                          size="icon-sm"
                                          aria-label={t("Edit {name}", {
                                            name: group.name,
                                          })}
                                          onClick={() =>
                                            navigate(
                                              `/admin/routing/channel-groups/${group.id}`,
                                            )
                                          }
                                        >
                                          <Pencil />
                                        </Button>
                                      </div>
                                    </CardAction>
                                  </CardHeader>
                                  <CardContent className="flex flex-wrap gap-2">
                                    <StatusBadge value={group.enabled} />
                                    <Badge variant="secondary">
                                      {t("Channels ({count})", {
                                        count: groupedChannels.length,
                                      })}
                                    </Badge>
                                    <Badge variant="outline">
                                      {t("Priority")}: {group.priority}
                                    </Badge>
                                    <Badge variant="outline">
                                      {selectionStrategyLabel(
                                        group.selection_strategy,
                                      )}
                                    </Badge>
                                  </CardContent>
                                </Card>
                              );
                            })}
                          </div>
                        </CardContent>
                      </Card>
                    );
                  })}
                </div>
              </section>
            ) : null}

            {standardEntries.length > 0 ? (
              <section
                aria-labelledby="standard-channel-groups"
                className="flex flex-col gap-3"
              >
                <div className="flex flex-wrap items-end justify-between gap-2">
                  <div className="flex flex-col gap-1">
                    <h2
                      id="standard-channel-groups"
                      className="text-lg font-semibold tracking-tight"
                    >
                      {t("Standard channel groups")}
                    </h2>
                    <p className="text-sm text-muted-foreground">
                      {t(
                        "Large lists stay compact until you expand a group to inspect its channels.",
                      )}
                    </p>
                  </div>
                  <Badge variant="secondary">{standardEntries.length}</Badge>
                </div>

                <div className="flex flex-col gap-3">
                  {standardEntries.map((entry) => (
                    <StandardGroupCard
                      key={entry.group.id}
                      group={entry.group}
                      channels={entry.channels}
                      preferOpen={preferExpandedStandardGroups}
                      onEdit={() =>
                        navigate(
                          `/admin/routing/channel-groups/${entry.group.id}`,
                        )
                      }
                      onDisable={() => setDisableGroupTarget(entry.group)}
                      disablePending={quickActionPending}
                    >
                      {channelTable(entry.channels)}
                    </StandardGroupCard>
                  ))}
                </div>
              </section>
            ) : null}

            {unavailableGroupChannels.length > 0 ? (
              <Card role="region" aria-labelledby="unavailable-channel-groups">
                <CardHeader>
                  <CardTitle id="unavailable-channel-groups">
                    {t("Unavailable channel groups")}
                  </CardTitle>
                  <CardDescription>
                    {t(
                      "These channels reference a group that is not available in the current response.",
                    )}
                  </CardDescription>
                </CardHeader>
                <CardContent>{channelTable(unavailableGroupChannels)}</CardContent>
              </Card>
            ) : null}
          </div>
        ) : (
          <EmptyState
            title={t("No matching channel groups")}
            description={t("Try another search or channel group type.")}
            className="min-h-48 border"
          />
        )}
      </AsyncResource>

      <ChannelBatchEditDialog
        open={batchOpen}
        channels={selectedChannels}
        onOpenChange={setBatchOpen}
        onApplied={() => setSelected(new Set())}
        triggerId={batchDialogTriggerId}
      />

      <ConfirmDialog
        open={disableGroupTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDisableGroupTarget(null);
        }}
        title={t("Disable channel group?")}
        description={
          disableGroupTarget
            ? t(
                "{name} will stop all {count} channels in this group from receiving new requests. Individual channel settings are preserved.",
                {
                  name: disableGroupTarget.name,
                  count: channelsByGroup.get(disableGroupTarget.id)?.length ?? 0,
                },
              )
            : ""
        }
        confirmLabel={t("Disable group")}
        destructive
        onConfirm={() => {
          const group = disableGroupTarget;
          setDisableGroupTarget(null);
          if (group) void disableGroup(group);
        }}
      />

      <ConfirmDialog
        open={disableTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDisableTarget(null);
        }}
        title={t("Disable channel?")}
        description={
          disableTarget
            ? t("{name} will stop receiving new requests.", {
                name: disableTarget.name,
              })
            : ""
        }
        confirmLabel={t("Disable")}
        destructive
        onConfirm={() => {
          const channel = disableTarget;
          setDisableTarget(null);
          if (channel) void setChannelEnabled(channel, false);
        }}
      />
    </div>
  );
}
