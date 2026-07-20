import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChannelGroups } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel, selectionStrategyLabel } from "@/lib/permissions";

export function ChannelGroupsPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useChannelGroups();
  return (
    <AdminListPage
      title="Channel Groups"
      description="Load-balancing pools for one API format with priority tiers."
      query={{ data, isLoading, error }}
      rowKey={(group) => group.id}
      detailPath={(group) => `/admin/routing/channel-groups/${group.id}`}
      createLabel="New group"
      onCreate={() => navigate("/admin/routing/channel-groups/new")}
      columns={[
        { key: "name", header: "Name", render: (group) => <span className="font-medium">{group.name}</span> },
        { key: "format", header: "Format", render: (group) => apiFormatLabel(group.api_format) },
        { key: "priority", header: "Priority", render: (group) => group.priority },
        {
          key: "strategy",
          header: "Strategy",
          render: (group) => selectionStrategyLabel(group.selection_strategy),
        },
        { key: "enabled", header: "Enabled", render: (group) => <StatusBadge value={group.enabled} /> },
        { key: "updated", header: "Updated", render: (group) => formatRelative(group.updated_at) },
      ]}
    />
  );
}
