import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useUpstreamAccesses } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";

export function AccessesPage() {
  const query = useUpstreamAccesses();
  const { t } = useI18n();
  return <AdminListPage
    title={t("Upstream accesses")}
    query={query}
    rowKey={(access) => access.id}
    rowName={(access) => access.name}
    searchText={(access) => `${access.name} ${access.connector_kind} ${access.base_url}`}
    detailPath={(access) => `/admin/routing/accesses/${access.id}`}
    createLabel={t("New access")}
    createPath="/admin/routing/accesses/new"
    columns={[
      { key: "name", header: t("Name"), render: (access) => access.name },
      { key: "connector", header: t("Connector"), render: (access) => access.connector_kind },
      { key: "base-url", header: "Base URL", render: (access) => access.base_url },
      { key: "enabled", header: t("Enabled"), render: (access) => <StatusBadge value={access.enabled} /> },
    ]}
  />;
}
