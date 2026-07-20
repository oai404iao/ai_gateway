import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useApiKeyPolicies } from "@/features/admin/api";
import { formatList } from "@/lib/formatters";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";

export function ApiKeyPoliciesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useApiKeyPolicies();
  return (
    <AdminListPage
      title="API Key Policies"
      description="Templates copied into self-service API keys to bound their authorization."
      query={{ data, isLoading, error }}
      rowKey={(policy) => policy.id}
      detailPath={(policy) => `/admin/api-key-policies/${policy.id}`}
      createLabel="New policy"
      onCreate={() => navigate("/admin/api-key-policies/new")}
      columns={[
        { key: "name", header: "Name", render: (policy) => <span className="font-medium">{policy.name}</span> },
        {
          key: "formats",
          header: "Formats",
          render: (policy) => (
            <span className="flex flex-wrap gap-1">
              {policy.allowed_api_formats.map((format) => (
                <StatusBadge
                  key={format}
                  value={format}
                  label={apiFormatLabel(format)}
                  variant="info"
                />
              ))}
            </span>
          ),
        },
        {
          key: "permissions",
          header: "Permissions",
          render: (policy) => formatList(policy.permissions),
        },
        { key: "maxkeys", header: "Max keys", render: (policy) => policy.max_active_keys },
        { key: "enabled", header: "Enabled", render: (policy) => <StatusBadge value={policy.enabled} /> },
        { key: "updated", header: "Updated", render: (policy) => formatRelative(policy.updated_at) },
      ]}
    />
  );
}
