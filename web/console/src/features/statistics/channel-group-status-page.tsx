import { PageHeader } from "@/components/shared/page-header";
import { ChannelGroupStatusPanel } from "@/features/statistics/channel-group-status-panel";

export function ChannelGroupStatusPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Channel group status" />
      <ChannelGroupStatusPanel />
    </div>
  );
}
