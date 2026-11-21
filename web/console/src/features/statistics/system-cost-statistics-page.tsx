import { PageHeader } from "@/components/shared/page-header";
import { Badge } from "@/components/ui/badge";
import { useI18n } from "@/app/i18n";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";

export function SystemCostStatisticsPage() {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Cost statistics" actions={<Badge variant="secondary">{t("Global")}</Badge>} />
      <CostStatisticsPanel scope="system" />
    </div>
  );
}
