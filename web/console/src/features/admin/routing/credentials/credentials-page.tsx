import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useUpstreamCredentials } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";

export function CredentialsPage() {
  const navigate = useNavigate();
  const query = useUpstreamCredentials();
  const { t } = useI18n();
  return <AdminListPage
    title={t("Upstream credentials")}
    description={t("Reusable upstream identities. Rotating a credential updates every referencing channel.")}
    query={query}
    rowKey={(credential) => credential.id}
    detailPath={(credential) => `/admin/routing/upstream-credentials/${credential.id}`}
    createLabel={t("New credential")}
    onCreate={() => navigate("/admin/routing/upstream-credentials/new")}
    columns={[
      { key: "name", header: t("Name"), render: (credential) => credential.name },
      { key: "kind", header: t("Type"), render: (credential) => credential.kind },
      { key: "channels", header: t("Channels"), render: (credential) => credential.channel_ids.length },
      { key: "enabled", header: t("Enabled"), render: (credential) => <StatusBadge value={credential.enabled} /> },
    ]}
  />;
}
