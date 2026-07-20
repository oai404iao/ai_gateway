import { useOwnRequestLogs } from "@/features/request-logs/api";
import { RequestLogsView } from "@/features/request-logs/request-logs-view";

export function OwnRequestLogsPage() {
  return (
    <RequestLogsView
      title="Request Logs"
      description="Your own proxied requests, usage, and settlement state."
      basePath="/me/request-logs"
      useLogs={useOwnRequestLogs}
    />
  );
}
