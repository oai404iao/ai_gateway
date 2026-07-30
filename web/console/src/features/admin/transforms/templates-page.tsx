import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useConfigTemplates } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export function ConfigTemplatesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useConfigTemplates();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Transform Templates")}
      description={t("Reusable constrained transform and network configuration documents.")}
      query={{ data, isLoading, error }}
      rowKey={(template) => template.id}
      detailPath={(template) => `/admin/transforms/templates/${template.id}`}
      createLabel={t("New template")}
      onCreate={() => navigate("/admin/transforms/templates/new")}
      columns={[
        {
          key: "name",
          header: t("Name"),
          render: (template) => <span className="font-medium">{template.name}</span>,
        },
        {
          key: "description",
          header: t("Description"),
          render: (template) => template.description ?? "—",
        },
        {
          key: "format",
          header: t("API format"),
          render: (template) =>
            template.api_format === null
              ? t("All formats")
              : apiFormatLabel(template.api_format),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (template) => <StatusBadge value={template.enabled} />,
        },
        {
          key: "updated",
          header: t("Updated"),
          render: (template) => formatRelative(template.updated_at),
        },
      ]}
    />
  );
}
