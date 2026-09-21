import { useMemo } from "react";
import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useChannelCapabilities, useLogicalChannels } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";
import { apiOperationLabel, capabilityTransportLabel } from "@/lib/permissions";

export function CapabilitiesPage() {
  const navigate = useNavigate();
  const query = useChannelCapabilities();
  const channels = useLogicalChannels();
  const { t } = useI18n();
  const channelNames = useMemo(
    () => new Map((channels.data ?? []).map((channel) => [channel.id, channel.name])),
    [channels.data],
  );
  return (
    <AdminListPage
      title={t("Channel capabilities")}
      description={t(
        "One operation, transport set, and model catalogue per logical channel.",
      )}
      query={query}
      rowKey={(capability) => capability.id}
      detailPath={(capability) => `/admin/routing/capabilities/${capability.id}`}
      createLabel={t("New capability")}
      onCreate={() => navigate("/admin/routing/capabilities/new")}
      columns={[
        {
          key: "channel",
          header: t("Channel"),
          render: (capability) =>
            channelNames.get(capability.channel_id) ?? capability.channel_id,
        },
        {
          key: "operation",
          header: t("Operation"),
          render: (capability) => apiOperationLabel(capability.settings.operation),
        },
        {
          key: "transports",
          header: t("Transports"),
          render: (capability) =>
            capability.settings.transports.map(capabilityTransportLabel).join(", "),
        },
        {
          key: "models",
          header: t("Models"),
          render: (capability) => String(capability.settings.available_models.length),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (capability) => (
            <StatusBadge value={capability.settings.enabled} />
          ),
        },
      ]}
    />
  );
}
