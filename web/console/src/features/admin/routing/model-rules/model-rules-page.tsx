import { useState } from "react";
import { useNavigate } from "react-router";
import { ListPlus } from "lucide-react";
import type { ApiFormat, ModelRuleView } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useModelRules, useModels } from "@/features/admin/api";
import { ModelRuleQuickAddDialog } from "@/features/admin/routing/model-rules/model-rule-quick-add-dialog";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

const FORMAT_ORDER: Record<ApiFormat, number> = {
  open_ai_chat_completions: 0,
  open_ai_responses: 1,
  open_ai_images: 2,
};

type GroupedModelRule = ModelRuleView & {
  modelDisplayName: string;
  modelGroupLabel: string;
};

export function ModelRulesPage() {
  const navigate = useNavigate();
  const rules = useModelRules();
  const models = useModels();
  const { t } = useI18n();
  const [quickAddOpen, setQuickAddOpen] = useState(false);
  const quickAddTriggerId = "model-rule-quick-add-trigger";
  const modelsById = new Map((models.data ?? []).map((model) => [model.id, model]));
  const groupedRules: GroupedModelRule[] = (rules.data ?? [])
    .map((rule) => {
      const model = modelsById.get(rule.upstream_model_id);
      const provider = model?.provider_name?.trim() || t("Unspecified provider");
      const sourceModelId = model?.source_model_id ?? rule.upstream_model;
      return {
        ...rule,
        modelDisplayName: model?.display_name ?? sourceModelId,
        modelGroupLabel: `${provider} · ${sourceModelId}`,
      };
    })
    .sort((left, right) => {
      const groupOrder = left.modelGroupLabel.localeCompare(right.modelGroupLabel);
      if (groupOrder !== 0) return groupOrder;
      const formatOrder = FORMAT_ORDER[left.api_format] - FORMAT_ORDER[right.api_format];
      return formatOrder || left.client_model.localeCompare(right.client_model);
    });

  return (
    <>
      <AdminListPage
        title={t("Model Rules")}
        description={t(
          "Map (client model, API format) to one priced upstream model and routing targets. Rules are grouped by upstream-model provider and model ID.",
        )}
        query={{
          data: groupedRules,
          isLoading: rules.isLoading || models.isLoading,
          error: rules.error ?? models.error,
        }}
        rowKey={(rule) => rule.id}
        detailPath={(rule) => `/admin/routing/model-rules/${rule.id}`}
        groupBy={(rule) => rule.modelGroupLabel}
        createLabel={t("New rule")}
        onCreate={() => navigate("/admin/routing/model-rules/new")}
        headerActions={
          <Button
            id={quickAddTriggerId}
            variant="outline"
            onClick={() => setQuickAddOpen(true)}
          >
            <ListPlus data-icon="inline-start" />
            {t("Quick add")}
          </Button>
        }
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
            render: (rule) => rule.modelDisplayName,
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
            key: "routing",
            header: t("Routing status"),
            render: (rule) => (
              <span className="flex flex-col gap-1">
                <StatusBadge value={rule.routing_status} />
                <span className="text-xs text-muted-foreground">
                  {t("Active {active} · Capable {capable} · Targets {target}", {
                    active: rule.active_channel_count,
                    capable: rule.model_capable_channel_count,
                    target: rule.target_channel_count,
                  })}
                </span>
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
      <ModelRuleQuickAddDialog
        open={quickAddOpen}
        onOpenChange={setQuickAddOpen}
        triggerId={quickAddTriggerId}
      />
    </>
  );
}
