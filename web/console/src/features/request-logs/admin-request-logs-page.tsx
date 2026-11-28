import { useAllRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";
import { useI18n } from "@/app/i18n";

export function AdminRequestLogsPage() {
  const { t } = useI18n();

  return (
    <RequestLogsView
      title={t("Global request logs")}
      basePath="/request-logs"
      useLogs={useAllRequestLogs}
      scope="system"
    />
  );
}
