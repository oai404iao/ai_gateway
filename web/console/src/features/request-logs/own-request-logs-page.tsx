import { useOwnApiKeys } from "@/features/api-keys/api";
import { useOwnRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";
import { useI18n } from "@/app/i18n";

export function OwnRequestLogsPage() {
  const { t } = useI18n();
  const apiKeys = useOwnApiKeys();

  return (
    <RequestLogsView
      title={t("Request Logs")}
      description={t("Your own proxied requests, usage, and settlement state.")}
      basePath="/me/request-logs"
      useLogs={useOwnRequestLogs}
      apiKeys={apiKeys.data ?? []}
    />
  );
}
