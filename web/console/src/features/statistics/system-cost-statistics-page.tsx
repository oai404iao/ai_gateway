import { PageHeader } from "@/components/shared/page-header";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";

export function SystemCostStatisticsPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Cost statistics"
        description="System-wide request activity, token usage, and cost analytics."
      />
      <CostStatisticsPanel scope="system" />
    </div>
  );
}
