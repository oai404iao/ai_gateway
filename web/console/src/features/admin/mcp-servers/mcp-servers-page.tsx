import { useNavigate } from "react-router";
import { Badge } from "@/components/ui/badge";
import { StatusBadge } from "@/components/shared/status-badge";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useMcpServers } from "@/features/admin/api";
import {
  mcpContextSizeLabel,
  mcpExternalAccessLabel,
  mcpImageQualityLabel,
  mcpKindLabel,
  mcpToolName,
} from "@/features/admin/mcp-servers/mcp-server-form";
import type {
  ImageMcpSettings,
  McpServerView,
  WebSearchMcpSettings,
} from "@/api/types";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export function McpServersPage() {
  const navigate = useNavigate();
  const query = useMcpServers();
  const { t } = useI18n();

  return (
    <AdminListPage
      title={t("MCP Servers")}
      description={t(
        "Manage public MCP endpoints backed by existing model rules and Gateway API keys.",
      )}
      query={query}
      rowKey={(server) => server.id}
      detailPath={(server) => `/admin/mcp-servers/${server.id}`}
      createLabel={t("New MCP server")}
      onCreate={() => navigate("/admin/mcp-servers/new")}
      columns={[
        {
          key: "server",
          header: t("MCP server"),
          render: (server) => (
            <span className="flex flex-col gap-1">
              <span className="font-medium">{server.name}</span>
              <span className="font-mono text-xs text-muted-foreground">
                /mcp/{server.slug}
              </span>
            </span>
          ),
        },
        {
          key: "kind",
          header: t("Tool"),
          render: (server) => (
            <span className="flex flex-col items-start gap-1">
              <StatusBadge
                value={server.kind}
                label={t(mcpKindLabel(server.kind))}
                variant="info"
              />
              <span className="font-mono text-xs text-muted-foreground">
                {mcpToolName(server.kind)}
              </span>
            </span>
          ),
        },
        {
          key: "model",
          header: t("Model rule"),
          render: (server) => (
            <span className="flex flex-col items-start gap-1">
              <span className="font-mono text-xs">{server.client_model}</span>
              <Badge variant="secondary">
                {apiFormatLabel(server.api_format)}
              </Badge>
            </span>
          ),
        },
        {
          key: "settings",
          header: t("Settings"),
          render: (server) => <SettingsSummary server={server} />,
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (server) => <StatusBadge value={server.enabled} />,
        },
        {
          key: "updated",
          header: t("Updated"),
          render: (server) => formatRelative(server.updated_at),
        },
      ]}
    />
  );
}

function SettingsSummary({ server }: { server: McpServerView }) {
  const { t } = useI18n();
  if (server.kind === "web_search") {
    const settings = server.settings as WebSearchMcpSettings;
    return (
      <span className="flex flex-wrap gap-1">
        <Badge variant="secondary">
          {t(mcpExternalAccessLabel(settings.external_web_access ?? "live"))}
        </Badge>
        <Badge variant="secondary">
          {t(mcpContextSizeLabel(settings.search_context_size ?? "medium"))}
        </Badge>
      </span>
    );
  }
  const settings = server.settings as ImageMcpSettings;
  return (
    <span className="flex flex-wrap gap-1">
      <Badge variant="secondary">{settings.size ?? "auto"}</Badge>
      <Badge variant="secondary">
        {t(mcpImageQualityLabel(settings.quality ?? "auto"))}
      </Badge>
    </span>
  );
}
