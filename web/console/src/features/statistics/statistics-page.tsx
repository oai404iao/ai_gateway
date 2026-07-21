import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageHeader } from "@/components/shared/page-header";
import { ChannelStatusPanel } from "@/features/statistics/channel-status-panel";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";
import { useI18n } from "@/app/i18n";

export function StatisticsPage() {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Statistics"
        description="Channel performance, reliability, usage, and cost analytics."
      />
      <Tabs defaultValue="channel-status">
        <TabsList>
          <TabsTrigger value="channel-status">{t("Channel status")}</TabsTrigger>
          <TabsTrigger value="costs">{t("Cost statistics")}</TabsTrigger>
        </TabsList>
        <TabsContent value="channel-status">
          <ChannelStatusPanel />
        </TabsContent>
        <TabsContent value="costs">
          <CostStatisticsPanel />
        </TabsContent>
      </Tabs>
    </div>
  );
}
