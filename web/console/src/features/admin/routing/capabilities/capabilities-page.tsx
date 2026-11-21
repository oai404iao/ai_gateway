import { useMemo, useState } from "react";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useChannelCapabilities, useLogicalChannels } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";
import { apiOperationLabel } from "@/lib/permissions";
import { Checkbox } from "@/components/ui/checkbox";
import { Button } from "@/components/ui/button";
import { CapabilityBatchDialog } from "./capability-batch-dialog";
import { AppendModelDialog } from "./append-model-dialog";
import type { ChannelCapabilityView } from "@/api/types";
import { useReturnPath } from "@/lib/page-navigation";
import { resourceLabels } from "@/lib/resource-labels";

export function CapabilitiesPage({ channelId }: { channelId?: string } = {}) {
  const query = useChannelCapabilities();
  const parentPath = useReturnPath("/admin/routing/channels");
  const channels = useLogicalChannels();
  const { t } = useI18n();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState(false);
  const [appendTargets, setAppendTargets] = useState<ChannelCapabilityView[] | null>(null);
  const visibleCapabilities = query.data?.filter((capability) => !channelId || capability.channel_id === channelId);
  const selectedCapabilities = (visibleCapabilities ?? []).filter((capability) => selected.has(capability.id));
  const invalidSelection = selectedCapabilities.length === 0 || selectedCapabilities.length > 100;
  const channelNames = useMemo(
    () => resourceLabels(channels.data ?? []),
    [channels.data],
  );
  return (
    <>
    <AdminListPage
      embedded={!!channelId}
      backTo={channelId ? undefined : parentPath}
      linkColumnKey="operation"
      title={t("Channel capabilities")}
      query={{
        data: visibleCapabilities,
        isLoading: query.isLoading || channels.isLoading,
        error: query.error ?? channels.error,
      }}
      rowKey={(capability) => capability.id}
      detailPath={(capability) => `/admin/routing/logical-channels/${capability.channel_id}?view=capabilities&capability=${capability.id}`}
      createLabel={t("New capability")}
      createPath={channelId
        ? `/admin/routing/logical-channels/${channelId}?view=capabilities&capability=new`
        : "/admin/routing/capabilities/new"}
      headerActions={<>
        <Button variant="outline" disabled={invalidSelection} onClick={() => setAppendTargets(selectedCapabilities)}>{t("Append upstream model")}</Button>
        <Button variant="outline" disabled={invalidSelection} onClick={() => setEditing(true)}>{t("Batch edit capabilities")} ({selectedCapabilities.length}/100)</Button>
      </>}
      columns={[
        {
          key: "select",
          header: t("Select"),
          render: (capability) => (
            <Checkbox
              aria-label={t("Select {name}", { name: `${channelNames.get(capability.channel_id) ?? t("Unknown channel")} / ${apiOperationLabel(capability.settings.operation)}` })}
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
            channelNames.get(capability.channel_id) ?? t("Unknown channel"),
        },
        {
          key: "operation",
          header: t("Operation"),
          render: (capability) => apiOperationLabel(capability.settings.operation),
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
    {appendTargets && <AppendModelDialog capabilities={appendTargets} channels={channels.data ?? []}
      onClose={() => setAppendTargets(null)} onApplied={() => setSelected(new Set())}
      refresh={async () => {
        const { data } = await query.refetch({ throwOnError: true });
        const ids = new Set(appendTargets.map(({ id }) => id));
        const next = (data ?? []).filter(({ id }) => ids.has(id));
        if (next.length !== ids.size) return false;
        setAppendTargets(next);
        return true;
      }} />}
    </>
  );
}
