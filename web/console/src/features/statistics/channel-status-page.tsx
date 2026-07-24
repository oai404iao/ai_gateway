import { PageHeader } from "@/components/shared/page-header";
import { useI18n } from "@/app/i18n";
import { ChannelStatusPanel } from "@/features/statistics/channel-status-panel";

export function ChannelStatusPage() {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Channel status")}
        description={t("Availability and performance for channels included in status statistics.")}
      />
      <ChannelStatusPanel />
    </div>
  );
}
