import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useOperationRules, useRoutingProfiles } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";
import { apiOperationLabel } from "@/lib/permissions";

export function OperationRulesPage() {
  const navigate = useNavigate();
  const query = useOperationRules();
  const profiles = useRoutingProfiles();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Operation rules")}
      description={t(
        "Per-operation routing tiers that reference channel capabilities and upstream wire models.",
      )}
      query={query}
      rowKey={(rule) => rule.id}
      detailPath={(rule) => `/admin/routing/operation-rules/${rule.id}`}
      createLabel={t("New operation rule")}
      onCreate={() => navigate("/admin/routing/operation-rules/new")}
      columns={[
        {
          key: "profile",
          header: t("Routing profile"),
          render: (rule) =>
            profiles.data?.find((profile) => profile.id === rule.model_routing_profile_id)
              ?.model_display_name ?? rule.model_routing_profile_id,
        },
        {
          key: "operation",
          header: t("Operation"),
          render: (rule) => apiOperationLabel(rule.operation),
        },
        {
          key: "tiers",
          header: t("Tiers"),
          render: (rule) => String(rule.routing_tiers.length),
        },
        {
          key: "candidates",
          header: t("Candidates"),
          render: (rule) =>
            String(
              rule.routing_tiers.reduce(
                (count, tier) => count + tier.candidates.length,
                0,
              ),
            ),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (rule) => <StatusBadge value={rule.enabled} />,
        },
      ]}
    />
  );
}
