import { PageHeader } from "@/components/shared/page-header";
import { useI18n } from "@/app/i18n";
import { ChannelGroupStatusPanel } from "@/features/statistics/channel-group-status-panel";

export function ChannelGroupStatusPage() {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Channel group status")}
        description={t(
          "Availability and performance aggregated for monitored channel groups.",
        )}
      />
      <ChannelGroupStatusPanel />
    </div>
  );
}
