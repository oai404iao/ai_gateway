import { useAdminApiKeys, useRoutingProfiles, useOperationRules, useUsers } from "@/features/admin/api";
import { useAllRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";
import { useI18n } from "@/app/i18n";

export function AdminRequestLogsPage() {
  const { t } = useI18n();
  const users = useUsers();
  const apiKeys = useAdminApiKeys();
  const profiles = useRoutingProfiles();
  const rules = useOperationRules();

  return (
    <RequestLogsView
      title={t("Request Logs")}
      description={t("All proxied requests across every user and API key.")}
      basePath="/request-logs"
      useLogs={useAllRequestLogs}
      scope="system"
      users={users.data ?? []}
      apiKeys={apiKeys.data ?? []}
      modelOptions={[
        ...(profiles.data ?? []).map((profile) => profile.client_model),
        ...(rules.data ?? []).flatMap((rule) => rule.routing_tiers.flatMap((tier) =>
          tier.candidates.map((candidate) => candidate.upstream_model))),
      ]}
    />
  );
}
