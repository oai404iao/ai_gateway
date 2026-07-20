import { useNavigate } from "react-router";
import { Badge } from "@/components/ui/badge";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useModelRules } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export function ModelRulesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useModelRules();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Model Rules")}
      description={t("Map (client model, API format) to one priced upstream model and routing targets.")}
      query={{ data, isLoading, error }}
      rowKey={(rule) => rule.id}
      detailPath={(rule) => `/admin/routing/model-rules/${rule.id}`}
      createLabel={t("New rule")}
      onCreate={() => navigate("/admin/routing/model-rules/new")}
      columns={[
        {
          key: "model",
          header: t("Client model"),
          render: (rule) => (
            <span className="flex flex-col">
              <span className="font-medium">{rule.client_model}</span>
              <StatusBadge
                value={rule.api_format}
                label={apiFormatLabel(rule.api_format)}
                variant="info"
                className="mt-1"
              />
            </span>
          ),
        },
        {
          key: "upstream",
          header: t("Upstream model"),
          render: (rule) => <span className="font-mono text-xs">{rule.upstream_model}</span>,
        },
        {
          key: "targets",
          header: t("Targets"),
          render: (rule) => (
            <span className="flex flex-wrap gap-1">
              <Badge variant="info">
                {t("{count} groups", { count: rule.channel_group_ids.length })}
              </Badge>
              <Badge variant="secondary">
                {t("{count} channels", { count: rule.channel_ids.length })}
              </Badge>
            </span>
          ),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (rule) => <StatusBadge value={rule.enabled} />,
        },
        { key: "updated", header: t("Updated"), render: (rule) => formatRelative(rule.updated_at) },
      ]}
    />
  );
}
