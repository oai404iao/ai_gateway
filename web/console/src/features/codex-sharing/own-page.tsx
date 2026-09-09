import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useOwnSharing } from "./api";
import { SharingUsage } from "./usage";

export function OwnSharingPage() {
  const query = useOwnSharing();
  const { t } = useI18n();
  const data = query.data;
  return <div className="flex flex-col gap-6">
    <PageHeader title="My Codex sharing" description="USD usage allowances, not account balance or official provider credits."
      actions={<Button variant="outline" disabled={query.isFetching} onClick={() => void query.refetch()}>
        {t("Refresh usage")}
      </Button>} />
    <AsyncResource isLoading={query.isLoading} error={query.error} isEmpty={!data}
      emptyTitle="No sharing membership">
      {data && <Card>
        <CardHeader>
          <CardTitle>{data.name}</CardTitle>
          <CardDescription>
            {t("Seat")}: {data.usage.seat_number ?? t("Unassigned")} / {data.seat_count}
            {" · "}{t(data.enabled ? "Enabled" : "Paused")}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <p className="text-sm text-muted-foreground">
            {t("Reserve per request (USD)")}: ${data.request_reservation_amount}
            {" · "}{t("User RPM")}: {data.user_requests_per_minute}
            {" · "}{t("User concurrency")}: {data.user_max_concurrent_requests}
          </p>
          <SharingUsage usage={data.usage} />
          <p className="text-sm text-muted-foreground">
            {t("All API keys share this allowance. Refreshing this page does not reset usage.")}
          </p>
        </CardContent>
      </Card>}
    </AsyncResource>
  </div>;
}
