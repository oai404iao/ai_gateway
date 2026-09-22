import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useChannelCapabilities, useLogicalChannels } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";
import { apiOperationLabel, capabilityTransportLabel } from "@/lib/permissions";
import { Checkbox } from "@/components/ui/checkbox";
import { Button } from "@/components/ui/button";
import { CapabilityBatchDialog } from "./capability-batch-dialog";

export function CapabilitiesPage() {
  const navigate = useNavigate();
  const query = useChannelCapabilities();
  const channels = useLogicalChannels();
  const { t } = useI18n();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState(false);
  const selectedCapabilities = (query.data ?? []).filter((capability) => selected.has(capability.id));
  const channelNames = useMemo(
    () => new Map((channels.data ?? []).map((channel) => [channel.id, channel.name])),
    [channels.data],
  );
  return (
    <>
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
      headerActions={<Button variant="outline" disabled={selectedCapabilities.length === 0 || selectedCapabilities.length > 100} onClick={() => setEditing(true)}>{t("Batch edit capabilities")} ({selectedCapabilities.length}/100)</Button>}
      columns={[
        {
          key: "select",
          header: t("Select"),
          render: (capability) => (
            <Checkbox
              aria-label={t("Select {name}", { name: `${channelNames.get(capability.channel_id) ?? capability.channel_id} / ${apiOperationLabel(capability.settings.operation)}` })}
              checked={selected.has(capability.id)}
              onClick={(event) => event.stopPropagation()}
              onCheckedChange={(checked) => setSelected((previous) => {
                const next = new Set(previous);
                if (checked) next.add(capability.id);
                else next.delete(capability.id);
                return next;
              })}
            />
          ),
        },
        {
          key: "auto_disabled",
          header: t("Automatically disabled"),
          render: (capability) => (
            <StatusBadge
              value={capability.auto_disabled}
              label={t(capability.auto_disabled ? "Yes" : "No")}
              variant={capability.auto_disabled ? "warning" : "secondary"}
            />
          ),
        },
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
    {editing && <CapabilityBatchDialog capabilities={selectedCapabilities} onClose={() => setEditing(false)} onApplied={() => setSelected(new Set())} />}
    </>
  );
}
