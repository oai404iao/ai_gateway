import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useProxies } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { formatList } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";

export function ProxiesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useProxies();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Proxies")}
      description={t("HTTP/SOCKS egress proxies reused by upstream reqwest clients.")}
      query={{ data, isLoading, error }}
      rowKey={(proxy) => proxy.id}
      detailPath={(proxy) => `/admin/network/proxies/${proxy.id}`}
      createLabel={t("New proxy")}
      onCreate={() => navigate("/admin/network/proxies/new")}
      columns={[
        {
          key: "name",
          header: t("Name"),
          render: (proxy) => <span className="font-medium">{proxy.name}</span>,
        },
        {
          key: "url",
          header: t("URL"),
          render: (proxy) => <span className="font-mono text-xs">{proxy.proxy_url}</span>,
        },
        {
          key: "hosts",
          header: t("No-proxy hosts"),
          render: (proxy) => formatList(proxy.no_proxy_hosts),
        },
        {
          key: "cred",
          header: t("Credential"),
          render: (proxy) => (proxy.credential_configured ? t("yes") : t("no")),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (proxy) => <StatusBadge value={proxy.enabled} />,
        },
        {
          key: "updated",
          header: t("Updated"),
          render: (proxy) => formatRelative(proxy.updated_at),
        },
      ]}
    />
  );
}
