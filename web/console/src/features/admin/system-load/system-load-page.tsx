import { PageHeader } from "@/components/shared/page-header";
import { SystemLoadPanel } from "@/features/admin/system-load/system-load-panel";

export function SystemLoadPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="System load"
        description="Current gateway instance resources and pipeline pressure."
      />
      <SystemLoadPanel />
    </div>
  );
}
