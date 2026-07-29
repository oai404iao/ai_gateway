import { useAdminApiKeys, useModelRules, useUsers } from "@/features/admin/api";
import { useAllRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";
import { useI18n } from "@/app/i18n";

export function AdminRequestLogsPage() {
  const { t } = useI18n();
  const users = useUsers();
  const apiKeys = useAdminApiKeys();
  const modelRules = useModelRules();

  return (
    <RequestLogsView
      title={t("Request Logs")}
      description={t("All proxied requests across every user and API key.")}
      basePath="/request-logs"
      useLogs={useAllRequestLogs}
      scope="system"
      users={users.data ?? []}
      apiKeys={apiKeys.data ?? []}
      modelOptions={
        modelRules.data?.flatMap((rule) => [rule.client_model, rule.upstream_model]) ?? []
      }
    />
  );
}
