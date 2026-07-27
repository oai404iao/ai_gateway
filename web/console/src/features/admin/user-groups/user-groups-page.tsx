import { useNavigate } from "react-router";
import { Badge } from "@/components/ui/badge";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useApiKeyPolicies, useUserGroups } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { roleLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export function UserGroupsPage() {
  const navigate = useNavigate();
  const groups = useUserGroups();
  const policies = useApiKeyPolicies();
  const { t } = useI18n();

  const policyName = (id: string | null) =>
    id ? policies.data?.find((policy) => policy.id === id)?.name ?? id : t("None");

  return (
    <AdminListPage
      title={t("User Groups")}
      description={t("Assign one group to each user and inherit its default API policy.")}
      query={groups}
      rowKey={(group) => group.id}
      detailPath={(group) => `/admin/user-groups/${group.id}`}
      createLabel={t("New user group")}
      onCreate={() => navigate("/admin/user-groups/new")}
      columns={[
        {
          key: "name",
          header: t("Name"),
          render: (group) => (
            <span className="flex items-center gap-2">
              <span className="font-medium">{group.name}</span>
              {group.system_role ? (
                <Badge variant="secondary">{t("Default for {role}", {
                  role: roleLabel(group.system_role),
                })}</Badge>
              ) : null}
            </span>
          ),
        },
        {
          key: "members",
          header: t("Members"),
          render: (group) => group.member_count,
        },
        {
          key: "policy",
          header: t("Default API key policy"),
          render: (group) => policyName(group.default_api_key_policy_id),
        },
        {
          key: "updated",
          header: t("Updated"),
          render: (group) => formatRelative(group.updated_at),
        },
      ]}
    />
  );
}
