import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useRoutingGroups } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";

export function GroupsPage({ embedded = false }: { embedded?: boolean } = {}) {
  const query = useRoutingGroups();
  const { t } = useI18n();
  return (
    <AdminListPage
      embedded={embedded}
      title={t("Channel groups")}
      description={t(
        "Canonical organization for logical channels. A group holds no format, connector, route, or grant.",
      )}
      query={query}
      rowKey={(group) => group.id}
      detailPath={(group) => `/admin/routing/groups/${group.id}`}
      createLabel={t("New group")}
      createPath="/admin/routing/groups/new"
      columns={[
        { key: "name", header: t("Name"), render: (group) => group.name },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (group) => <StatusBadge value={group.enabled} />,
        },
      ]}
    />
  );
}
