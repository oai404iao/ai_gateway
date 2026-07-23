import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageHeader } from "@/components/shared/page-header";
import { ChannelStatusPanel } from "@/features/statistics/channel-status-panel";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";
import { SystemLoadPanel } from "@/features/statistics/system-load-panel";
import { useI18n } from "@/app/i18n";
import { useSession } from "@/lib/use-session";

export function StatisticsPage() {
  const { t } = useI18n();
  const { user } = useSession();
  const isAdmin = user?.role === "admin";

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Statistics"
        description={
          isAdmin
            ? "Channel, cost, and current system pressure analytics."
            : "Channel status and your own cost analytics."
        }
      />
      <Tabs defaultValue="channel-status">
        <TabsList>
          <TabsTrigger value="channel-status">{t("Channel status")}</TabsTrigger>
          <TabsTrigger value="costs">{t("Cost statistics")}</TabsTrigger>
          {isAdmin ? (
            <TabsTrigger value="system-load">{t("System load")}</TabsTrigger>
          ) : null}
        </TabsList>
        <TabsContent value="channel-status">
          <ChannelStatusPanel />
        </TabsContent>
        <TabsContent value="costs">
          <CostStatisticsPanel />
        </TabsContent>
        {isAdmin ? (
          <TabsContent value="system-load">
            <SystemLoadPanel />
          </TabsContent>
        ) : null}
      </Tabs>
    </div>
  );
}
