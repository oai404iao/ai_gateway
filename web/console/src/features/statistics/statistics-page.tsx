import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageHeader } from "@/components/shared/page-header";
import { Badge } from "@/components/ui/badge";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";
import { PersonalUsagePanel } from "@/features/statistics/personal-usage-panel";
import { useI18n } from "@/app/i18n";

export function StatisticsPage() {
  const { t } = useI18n();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Usage report" actions={<Badge variant="secondary">{t("Personal")}</Badge>} />
      <Tabs defaultValue="costs">
        <TabsList variant="line">
          <TabsTrigger value="costs">{t("Cost statistics")}</TabsTrigger>
          <TabsTrigger value="personal-usage">{t("Personal usage")}</TabsTrigger>
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
