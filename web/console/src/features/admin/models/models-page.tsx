import { useNavigate } from "react-router";
import { Calculator, Copy, Workflow } from "lucide-react";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/shared/status-badge";
import { useModels } from "@/features/admin/api";
import { groupModelsByProvider } from "@/features/admin/models/model-groups";
import { formatRelative } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";
import {
  MODEL_SETUP_PATH,
  adminPath,
} from "@/features/admin/model-setup/model-setup-navigation";

export function ModelsPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useModels();
  const { t } = useI18n();
  const providerGroups = groupModelsByProvider(data ?? [], t("Unspecified provider"));
  const groupedModels = providerGroups.flatMap((group) => group.models);
  return (
    <AdminListPage
      title={t("Upstream Models")}
      description={t("Upstream model identifiers and their USD prices. Prices carry an effective timestamp.")}
      query={{ data: groupedModels, isLoading, error }}
      rowKey={(model) => model.id}
      detailPath={(model) => `/admin/models/${model.id}`}
      groupBy={(model) => model.provider_name?.trim() || t("Unspecified provider")}
      createLabel={t("New upstream model")}
      onCreate={() => navigate("/admin/models/new")}
      headerActions={
        <Button
          variant="outline"
          onClick={() => navigate(MODEL_SETUP_PATH)}
        >
          <Workflow data-icon="inline-start" />
          {t("Guided model setup")}
        </Button>
      }
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
        {
          key: "pricing",
          header: t("Actions"),
          className: "w-0 text-right",
          render: (model) => (
            <div className="flex justify-end gap-1">
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={t("Copy {name}", {
                  name: model.display_name,
                })}
                onClick={() =>
                  navigate(
                    adminPath("/admin/models/new", {
                      copyFrom: model.id,
                      returnTo: "/admin/models",
                    }),
                  )
                }
              >
                <Copy />
              </Button>
              <Button
                variant="outline"
                size="sm"
                aria-label={t("Configure pricing for {model}", {
                  model: model.display_name,
                })}
                onClick={() => navigate(`/admin/models/${model.id}/pricing`)}
              >
                <Calculator data-icon="inline-start" />
                {t("Configure pricing")}
              </Button>
            </div>
          ),
        },
      ]}
    />
  );
}
