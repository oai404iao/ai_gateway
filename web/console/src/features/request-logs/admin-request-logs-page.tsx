import { useAllRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";

export function AdminRequestLogsPage() {
  return (
    <RequestLogsView
      title="Request Logs"
      description="All proxied requests across every user and API key."
      basePath="/request-logs"
      useLogs={useAllRequestLogs}
    />
  );
}
