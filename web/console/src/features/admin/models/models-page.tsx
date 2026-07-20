import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useModels } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";

export function ModelsPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useModels();
  const { t } = useI18n();
  return (
    <AdminListPage
      title={t("Upstream Models")}
      description={t("Upstream model identifiers and their prices. Prices carry an effective timestamp.")}
      query={{ data, isLoading, error }}
      rowKey={(model) => model.id}
      detailPath={(model) => `/admin/models/${model.id}`}
      createLabel={t("New upstream model")}
      onCreate={() => navigate("/admin/models/new")}
      columns={[
        {
          key: "name",
          header: t("Model"),
          render: (model) => (
            <span className="flex flex-col">
              <span className="font-medium">{model.display_name}</span>
              <span className="text-xs text-muted-foreground font-mono">{model.source_model_id}</span>
            </span>
          ),
        },
        { key: "provider", header: t("Provider"), render: (model) => model.provider_name ?? "—" },
        { key: "currency", header: t("Currency"), render: (model) => model.currency },
        {
          key: "input",
          header: t("Input price"),
          render: (model) => formatDecimal(model.input_unit_price),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (model) => <StatusBadge value={model.enabled} />,
        },
        { key: "updated", header: t("Updated"), render: (model) => formatRelative(model.updated_at) },
      ]}
    />
  );
}
