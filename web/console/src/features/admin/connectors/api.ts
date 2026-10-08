import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type { ConnectorPluginView } from "@/api/types";

export function useConnectorPlugins() {
  return useQuery({
    queryKey: ["admin", "connector-plugins"],
    queryFn: ({ signal }) =>
      apiGet<ConnectorPluginView[]>("/system/connectors", signal),
    staleTime: 60_000,
  });
}
