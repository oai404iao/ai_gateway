import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useModels } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";

export function ModelsPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useModels();
  return (
    <AdminListPage
      title="Models"
      description="Priced models referenced by model rules. Prices carry an effective timestamp."
      query={{ data, isLoading, error }}
      rowKey={(model) => model.id}
      detailPath={(model) => `/admin/models/${model.id}`}
      createLabel="New model"
      onCreate={() => navigate("/admin/models/new")}
      columns={[
        {
          key: "name",
          header: "Model",
          render: (model) => (
            <span className="flex flex-col">
              <span className="font-medium">{model.display_name}</span>
              <span className="text-xs text-muted-foreground font-mono">{model.source_model_id}</span>
            </span>
          ),
        },
        { key: "provider", header: "Provider", render: (model) => model.provider_name ?? "—" },
        { key: "currency", header: "Currency", render: (model) => model.currency },
        {
          key: "input",
          header: "Input price",
          render: (model) => formatDecimal(model.input_unit_price),
        },
        { key: "enabled", header: "Enabled", render: (model) => <StatusBadge value={model.enabled} /> },
        { key: "updated", header: "Updated", render: (model) => formatRelative(model.updated_at) },
      ]}
    />
  );
}
