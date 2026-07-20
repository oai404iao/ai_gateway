import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useProxies } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { formatList } from "@/lib/formatters";

export function ProxiesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useProxies();
  return (
    <AdminListPage
      title="Proxies"
      description="HTTP/SOCKS egress proxies reused by upstream reqwest clients."
      query={{ data, isLoading, error }}
      rowKey={(proxy) => proxy.id}
      detailPath={(proxy) => `/admin/network/proxies/${proxy.id}`}
      createLabel="New proxy"
      onCreate={() => navigate("/admin/network/proxies/new")}
      columns={[
        { key: "name", header: "Name", render: (proxy) => <span className="font-medium">{proxy.name}</span> },
        { key: "url", header: "URL", render: (proxy) => <span className="font-mono text-xs">{proxy.proxy_url}</span> },
        {
          key: "hosts",
          header: "No-proxy hosts",
          render: (proxy) => formatList(proxy.no_proxy_hosts),
        },
        { key: "cred", header: "Credential", render: (proxy) => (proxy.credential_configured ? "yes" : "no") },
        { key: "enabled", header: "Enabled", render: (proxy) => <StatusBadge value={proxy.enabled} /> },
        { key: "updated", header: "Updated", render: (proxy) => formatRelative(proxy.updated_at) },
      ]}
    />
  );
}
