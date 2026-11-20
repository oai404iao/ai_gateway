import { useQuery } from "@tanstack/react-query";
import { apiGet } from "@/api/client";
import type { ConnectorPluginView } from "@/api/types";

export const CONNECTORS_KEY = ["admin", "connector-plugins"] as const;

export function useConnectorPlugins() {
  return useQuery({
    queryKey: CONNECTORS_KEY,
    queryFn: ({ signal }) =>
      apiGet<ConnectorPluginView[]>("/system/connectors", signal),
    staleTime: 60_000,
  });
}
