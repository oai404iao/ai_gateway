import { useSearchParams } from "react-router";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/app/i18n";
import { CostStatisticsPanel } from "@/features/statistics/cost-statistics-panel";

export function SystemCostStatisticsPage() {
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Cost statistics" actions={<Badge variant="secondary">{t("Global")}</Badge>} />
      {params.has("codex_credential_id") ? (
        <Alert>
          <AlertTitle>{t("Credential filters are no longer supported.")}</AlertTitle>
          <AlertDescription className="flex flex-col items-start gap-2">
            {t("Remove the credential filter to view costs for the remaining filters.")}
            <Button variant="outline" onClick={() => {
              const next = new URLSearchParams(params);
              next.delete("codex_credential_id");
              setParams(next, { replace: true });
            }}>{t("Remove credential filter")}</Button>
          </AlertDescription>
        </Alert>
      ) : <CostStatisticsPanel scope="system" />}
    </div>
  );
}
