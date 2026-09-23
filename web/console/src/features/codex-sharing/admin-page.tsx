import { useNavigate } from "react-router";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { NavigationLink } from "@/components/shared/navigation-link";
import { ResourceTable } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";
import { useListPagination } from "@/lib/use-list-pagination";
import { useSharingGroups } from "./api";

export function SharingGroupsPage() {
  const query = useSharingGroups();
  const { t } = useI18n();
  const origin = usePageOrigin();
  const navigate = useNavigate();
  const pagination = useListPagination();
  return <div className="flex flex-col gap-6">
    <PageHeader title="Codex sharing" description="Dedicated credentials, fixed seats and provider-aligned USD windows."
      actions={<NavigationLink variant="default" size="default" to={withReturnTo("/admin/codex-sharing/new", origin)}>{t("New sharing group")}</NavigationLink>} />
    {query.data && !query.data.runtime_available && <Alert><AlertDescription>
      {t("Enable codex_sharing.enabled on one gateway instance before enabling a sharing group.")}
    </AlertDescription></Alert>}
    <AsyncResource isLoading={query.isLoading} error={query.error} isEmpty={query.data?.groups.length === 0}>
      <ResourceTable rows={query.data?.groups ?? []} rowKey={(group) => group.id}
        rowHref={(group) => withReturnTo(`/admin/codex-sharing/${group.id}`, origin)}
        onRowClick={(group) => navigate(withReturnTo(`/admin/codex-sharing/${group.id}`, origin))}
        pagination={pagination}
        columns={[
          { key: "name", header: t("Name"), render: (group) => group.name },
          { key: "seats", header: t("Seats"), render: (group) => `${group.seats.filter(Boolean).length} / ${group.seats.length}` },
          { key: "primary", header: t("Primary window"), render: (group) => `$${group.primary_limit_amount}` },
          { key: "secondary", header: t("Secondary window"), render: (group) => `$${group.secondary_limit_amount}` },
          { key: "enabled", header: t("Status"), render: (group) => <StatusBadge value={group.enabled} label={t(group.enabled ? "Enabled" : "Paused")} /> },
        ]} />
    </AsyncResource>
  </div>;
}
