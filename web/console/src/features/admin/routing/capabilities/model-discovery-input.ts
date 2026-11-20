import type { ApiOperation, ChannelModelDiscoveryInput, LogicalChannelView, UpstreamAccessView } from "@/api/types";
import { operationApiFormat } from "@/lib/permissions";

export function supportsModelDiscovery(access: UpstreamAccessView) {
  return access.connector_kind === "general" || access.connector_kind === "codex";
}

export function modelDiscoveryInput(
  channel: LogicalChannelView, access: UpstreamAccessView, operation: ApiOperation,
  template: string | null, overrides: ChannelModelDiscoveryInput["override_document"],
): ChannelModelDiscoveryInput {
  return {
    api_format: operationApiFormat(operation), base_url: access.base_url,
    credential_id: channel.credential_id, proxy_id: access.proxy_id,
    connect_timeout_ms: access.connect_timeout_ms,
    response_header_timeout_ms: access.response_header_timeout_ms,
    stream_idle_timeout_ms: access.stream_idle_timeout_ms,
    config_template_id: template, override_document: overrides,
  };
}
