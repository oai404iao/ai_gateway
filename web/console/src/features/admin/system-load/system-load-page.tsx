import { PageHeader } from "@/components/shared/page-header";
import { SystemLoadPanel } from "@/features/admin/system-load/system-load-panel";

export function SystemLoadPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="System load" />
      <SystemLoadPanel />
    </div>
  );
}
