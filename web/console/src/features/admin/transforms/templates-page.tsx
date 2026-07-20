import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { StatusBadge } from "@/components/shared/status-badge";
import { useConfigTemplates } from "@/features/admin/api";
import { formatRelative } from "@/lib/dates";

export function ConfigTemplatesPage() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useConfigTemplates();
  return (
    <AdminListPage
      title="Transform Templates"
      description="Reusable constrained transform and network configuration documents."
      query={{ data, isLoading, error }}
      rowKey={(template) => template.id}
      detailPath={(template) => `/admin/transforms/templates/${template.id}`}
      createLabel="New template"
      onCreate={() => navigate("/admin/transforms/templates/new")}
      columns={[
        { key: "name", header: "Name", render: (template) => <span className="font-medium">{template.name}</span> },
        { key: "description", header: "Description", render: (template) => template.description ?? "—" },
        { key: "enabled", header: "Enabled", render: (template) => <StatusBadge value={template.enabled} /> },
        { key: "updated", header: "Updated", render: (template) => formatRelative(template.updated_at) },
      ]}
    />
  );
}
