import { useOwnRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";
import { useI18n } from "@/app/i18n";

export function OwnRequestLogsPage() {
  const { t } = useI18n();
  return (
    <RequestLogsView
      title={t("Request Logs")}
      description={t("Your own proxied requests, usage, and settlement state.")}
      basePath="/me/request-logs"
      useLogs={useOwnRequestLogs}
    />
  );
}
