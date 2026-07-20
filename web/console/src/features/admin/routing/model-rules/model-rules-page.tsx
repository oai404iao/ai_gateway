import { useNavigate } from "react-router";
import { Badge } from "@/components/ui/badge";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useModelRules } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";

export function ModelRulesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useModelRules();
  return (
    <AdminListPage
      title="Model Rules"
      description="Map (client model, API format) to a priced model and routing targets."
      query={{ data, isLoading, error }}
      rowKey={(rule) => rule.id}
      detailPath={(rule) => `/admin/routing/model-rules/${rule.id}`}
      createLabel="New rule"
      onCreate={() => navigate("/admin/routing/model-rules/new")}
      columns={[
        {
          key: "model",
          header: "Client model",
          render: (rule) => (
            <span className="flex flex-col">
              <span className="font-medium">{rule.client_model}</span>
              <span className="text-xs text-muted-foreground">{apiFormatLabel(rule.api_format)}</span>
            </span>
          ),
        },
        { key: "upstream", header: "Upstream model", render: (rule) => <span className="font-mono text-xs">{rule.upstream_model}</span> },
        {
          key: "targets",
          header: "Targets",
          render: (rule) => (
            <span className="flex flex-wrap gap-1">
              <Badge variant="secondary">{rule.channel_group_ids.length} groups</Badge>
              <Badge variant="secondary">{rule.channel_ids.length} channels</Badge>
            </span>
          ),
        },
        { key: "enabled", header: "Enabled", render: (rule) => <StatusBadge value={rule.enabled} /> },
        { key: "updated", header: "Updated", render: (rule) => formatRelative(rule.updated_at) },
      ]}
    />
  );
}
