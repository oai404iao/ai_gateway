import { useMemo } from "react";
import { useNavigate } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import {
  useLogicalChannels,
  useRoutingGroups,
  useUpstreamAccesses,
  useUpstreamCredentials,
} from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";

export function LogicalChannelsPage() {
  const navigate = useNavigate();
  const query = useLogicalChannels();
  const groups = useRoutingGroups();
  const accesses = useUpstreamAccesses();
  const credentials = useUpstreamCredentials();
  const { t } = useI18n();
  const groupNames = useMemo(
    () => new Map((groups.data ?? []).map((group) => [group.id, group.name])),
    [groups.data],
  );
  const accessNames = useMemo(
    () => new Map((accesses.data ?? []).map((access) => [access.id, access.name])),
    [accesses.data],
  );
  const credentialNames = useMemo(
    () => new Map((credentials.data ?? []).map((credential) => [credential.id, credential.name])),
    [credentials.data],
  );
  return (
    <AdminListPage
      title={t("Logical channels")}
      description={t(
        "Each channel binds one group, one upstream access, and at most one credential.",
      )}
      query={query}
      rowKey={(channel) => channel.id}
      detailPath={(channel) => `/admin/routing/logical-channels/${channel.id}`}
      createLabel={t("New channel")}
      onCreate={() => navigate("/admin/routing/logical-channels/new")}
      columns={[
        { key: "name", header: t("Name"), render: (channel) => channel.name },
        {
          key: "group",
          header: t("Group"),
          render: (channel) => groupNames.get(channel.group_id) ?? channel.group_id,
        },
        {
          key: "access",
          header: t("Access"),
          render: (channel) => accessNames.get(channel.access_id) ?? channel.access_id,
        },
        {
          key: "credential",
          header: t("Credential"),
          render: (channel) =>
            channel.credential_id
              ? credentialNames.get(channel.credential_id) ?? channel.credential_id
              : t("No authentication"),
        },
        {
          key: "enabled",
          header: t("Enabled"),
          render: (channel) => <StatusBadge value={channel.enabled} />,
        },
      ]}
    />
  );
}
