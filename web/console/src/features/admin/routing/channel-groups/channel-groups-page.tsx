import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useChannelGroups } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel, selectionStrategyLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export function ChannelGroupsPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useChannelGroups();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Channel Groups")}
      description={t("Load-balancing pools for one API format with priority tiers.")}
      query={{ data, isLoading, error }}
      rowKey={(group) => group.id}
      detailPath={(group) => `/admin/routing/channel-groups/${group.id}`}
      createLabel={t("New group")}
      onCreate={() => navigate("/admin/routing/channel-groups/new")}
      columns={[
        {
          key: "name",
          header: t("Name"),
          render: (group) => <span className="font-medium">{group.name}</span>,
        },
        {
          key: "format",
          header: t("Format"),
          render: (group) => (
            <StatusBadge
              value={group.api_format}
              label={apiFormatLabel(group.api_format)}
              variant="info"
            />
          ),
        },
        { key: "priority", header: t("Priority"), render: (group) => group.priority },
        {
          key: "strategy",
          header: t("Strategy"),
          render: (group) => (
            <StatusBadge
              value={group.selection_strategy}
              label={selectionStrategyLabel(group.selection_strategy)}
              variant="secondary"
            />
          ),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (group) => <StatusBadge value={group.enabled} />,
        },
        { key: "updated", header: t("Updated"), render: (group) => formatRelative(group.updated_at) },
      ]}
    />
  );
}
