import { useNavigate } from "react-router";
import { Badge } from "@/components/ui/badge";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChannelGroups, useChannels } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";

export function ChannelsPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useChannels();
  const groups = useChannelGroups();
  const groupName = (id: string) =>
    groups.data?.find((group) => group.id === id)?.name ?? id.slice(0, 8);

  return (
    <AdminListPage
      title="Channels"
      description="Upstream endpoints inside a channel group with weight, timeouts, and auth."
      query={{ data, isLoading, error }}
      rowKey={(channel) => channel.id}
      detailPath={(channel) => `/admin/routing/channels/${channel.id}`}
      createLabel="New channel"
      onCreate={() => navigate("/admin/routing/channels/new")}
      columns={[
        { key: "name", header: "Name", render: (channel) => <span className="font-medium">{channel.name}</span> },
        { key: "group", header: "Group", render: (channel) => groupName(channel.channel_group_id) },
        { key: "format", header: "Format", render: (channel) => apiFormatLabel(channel.api_format) },
        {
          key: "state",
          header: "State",
          render: (channel) => (
            <div className="flex items-center gap-1">
              <StatusBadge value={channel.enabled} />
              {channel.auto_disabled ? <Badge variant="destructive">auto-disabled</Badge> : null}
            </div>
          ),
        },
        { key: "weight", header: "Weight", render: (channel) => channel.weight },
        { key: "updated", header: "Updated", render: (channel) => formatRelative(channel.updated_at) },
      ]}
    />
  );
}
