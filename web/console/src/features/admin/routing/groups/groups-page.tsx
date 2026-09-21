import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useRoutingGroups } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";

export function GroupsPage() {
  const navigate = useNavigate();
  const query = useRoutingGroups();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Routing groups")}
      description={t(
        "Canonical organization for logical channels. A group holds no format, connector, route, or grant.",
      )}
      query={query}
      rowKey={(group) => group.id}
      detailPath={(group) => `/admin/routing/groups/${group.id}`}
      createLabel={t("New group")}
      onCreate={() => navigate("/admin/routing/groups/new")}
      columns={[
        { key: "name", header: t("Name"), render: (group) => group.name },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (group) => <StatusBadge value={group.enabled} />,
        },
        {
          key: "sharing",
          header: t("Sharing only"),
          render: (group) => <StatusBadge value={group.sharing_only} />,
        },
      ]}
    />
  );
}
