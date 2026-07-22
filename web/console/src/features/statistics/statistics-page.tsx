import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageHeader } from "@/components/shared/page-header";
import { ChannelStatusPanel } from "@/features/statistics/channel-status-panel";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";
import { SystemLoadPanel } from "@/features/statistics/system-load-panel";
import { useI18n } from "@/app/i18n";

export function StatisticsPage() {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Statistics"
        description="Channel, cost, and current system pressure analytics."
      />
      <Tabs defaultValue="channel-status">
        <TabsList>
          <TabsTrigger value="channel-status">{t("Channel status")}</TabsTrigger>
          <TabsTrigger value="costs">{t("Cost statistics")}</TabsTrigger>
          <TabsTrigger value="system-load">{t("System load")}</TabsTrigger>
        </TabsList>
        <TabsContent value="channel-status">
          <ChannelStatusPanel />
        </TabsContent>
        <TabsContent value="costs">
          <CostStatisticsPanel />
        </TabsContent>
        <TabsContent value="system-load">
          <SystemLoadPanel />
        </TabsContent>
      </Tabs>
    </div>
  );
}
