import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageHeader } from "@/components/shared/page-header";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";
import { PersonalUsagePanel } from "@/features/statistics/personal-usage-panel";
import { useI18n } from "@/app/i18n";

export function StatisticsPage() {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Statistics"
        description="Personal request activity and cost analytics."
      />
      <Tabs defaultValue="personal-usage">
        <TabsList>
          <TabsTrigger value="personal-usage">{t("Personal usage")}</TabsTrigger>
          <TabsTrigger value="costs">{t("Cost statistics")}</TabsTrigger>
        </TabsList>
        <TabsContent value="personal-usage">
          <PersonalUsagePanel />
        </TabsContent>
        <TabsContent value="costs">
          <CostStatisticsPanel scope="own" />
        </TabsContent>
      </Tabs>
    </div>
  );
}
