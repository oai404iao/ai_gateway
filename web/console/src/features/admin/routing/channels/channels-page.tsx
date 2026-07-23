import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { CheckCheck, ListChecks, Pencil, Plus } from "lucide-react";
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
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChannelGroups, useChannels } from "@/features/admin/api";
import { ChannelBatchEditDialog } from "@/features/admin/routing/channels/channel-batch-edit-dialog";
import { formatRelative } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";
import { apiFormatLabel, selectionStrategyLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";
import type { ChannelView } from "@/api/types";

export function ChannelsPage() {
  const navigate = useNavigate();
  const groups = useChannelGroups();
  const channels = useChannels();
  const { t } = useI18n();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [batchOpen, setBatchOpen] = useState(false);
  const batchDialogTriggerId = "channel-batch-edit-trigger";

  useEffect(() => {
    const available = new Set((channels.data ?? []).map((channel) => channel.id));
    setSelected((current) => {
      const next = new Set([...current].filter((id) => available.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [channels.data]);

  const channelsByGroup = useMemo(() => {
    const grouped = new Map<string, ChannelView[]>();
    for (const channel of channels.data ?? []) {
      const current = grouped.get(channel.channel_group_id) ?? [];
      grouped.set(channel.channel_group_id, [...current, channel]);
    }
    return grouped;
  }, [channels.data]);

  const knownGroupIds = useMemo(
    () => new Set((groups.data ?? []).map((group) => group.id)),
    [groups.data],
  );
  const unavailableGroupChannels = (channels.data ?? []).filter(
    (channel) => !knownGroupIds.has(channel.channel_group_id),
  );
  const selectedChannels = useMemo(
    () => (channels.data ?? []).filter((channel) => selected.has(channel.id)),
    [channels.data, selected],
  );
  const allSelected =
    (channels.data?.length ?? 0) > 0 &&
    selectedChannels.length === channels.data?.length;

  const toggleChannel = (channel: ChannelView) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(channel.id)) next.delete(channel.id);
      else next.add(channel.id);
      return next;
    });
  };

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
      render: (channel) => (
        <div className="flex items-center gap-1">
          <StatusBadge value={channel.enabled} />
          {channel.auto_disabled ? (
            <Badge variant="destructive">{t("auto-disabled")}</Badge>
          ) : null}
        </div>
      ),
    },
    {
      key: "statistics",
      header: t("Status statistics"),
      render: (channel) => <StatusBadge value={channel.status_statistics_enabled} />,
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

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Channels")}
        description={t("Manage channel groups and their upstream channels in one grouped view.")}
        actions={
          <>
            <Button
              variant="outline"
              disabled={(channels.data?.length ?? 0) === 0}
              onClick={() => {
                if (allSelected) setSelected(new Set());
                else setSelected(new Set((channels.data ?? []).map((channel) => channel.id)));
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

      <AsyncResource
        isLoading={groups.isLoading || channels.isLoading}
        error={groups.error ?? channels.error}
        isEmpty={(groups.data?.length ?? 0) === 0 && (channels.data?.length ?? 0) === 0}
        emptyTitle={t("No records")}
        emptyDescription={t("There are no records to show yet.")}
      >
        <div className="flex flex-col gap-4">
          {(groups.data ?? []).map((group) => {
            const groupedChannels = channelsByGroup.get(group.id) ?? [];
            const titleId = `channel-group-${group.id}`;
            return (
              <Card key={group.id} role="region" aria-labelledby={titleId}>
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
                        {t("Channels ({count})", { count: groupedChannels.length })}
                      </Badge>
                      <Badge variant="outline">
                        {t("Priority")}: {group.priority}
                      </Badge>
                      <Badge variant="outline">
                        {selectionStrategyLabel(group.selection_strategy)}
                      </Badge>
                    </span>
                  </CardDescription>
                  <CardAction>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        navigate(`/admin/routing/channel-groups/${group.id}`)
                      }
                    >
                      <Pencil data-icon="inline-start" /> {t("Edit group")}
                    </Button>
                  </CardAction>
                </CardHeader>
                <CardContent>{channelTable(groupedChannels)}</CardContent>
              </Card>
            );
          })}

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
      </AsyncResource>

      <ChannelBatchEditDialog
        open={batchOpen}
        channels={selectedChannels}
        onOpenChange={setBatchOpen}
        onApplied={() => setSelected(new Set())}
        triggerId={batchDialogTriggerId}
      />
    </div>
  );
}
