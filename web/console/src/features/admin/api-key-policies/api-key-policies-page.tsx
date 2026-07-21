import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useApiKeyPolicies } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { useI18n } from "@/app/i18n";

export function ApiKeyPoliciesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useApiKeyPolicies();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("API Key Policies")}
      description={t("Defines the channel groups and channels users may assign to their keys.")}
      query={{ data, isLoading, error }}
      rowKey={(policy) => policy.id}
      detailPath={(policy) => `/admin/api-key-policies/${policy.id}`}
      createLabel={t("New policy")}
      onCreate={() => navigate("/admin/api-key-policies/new")}
      columns={[
        {
          key: "name",
          header: t("Name"),
          render: (policy) => <span className="font-medium">{policy.name}</span>,
        },
        {
          key: "groups",
          header: t("Channel groups"),
          render: (policy) => policy.allowed_group_ids.length,
        },
        {
          key: "channels",
          header: t("Individual channels"),
          render: (policy) => policy.allowed_channel_ids.length,
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (policy) => <StatusBadge value={policy.enabled} />,
        },
        {
          key: "updated",
          header: t("Updated"),
          render: (policy) => formatRelative(policy.updated_at),
        },
      ]}
    />
  );
}
