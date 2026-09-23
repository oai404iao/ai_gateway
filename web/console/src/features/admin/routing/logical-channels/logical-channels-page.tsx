import { useMemo } from "react";
import { useSearchParams } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import {
  useLogicalChannels,
  useRoutingGroups,
  useUpstreamAccesses,
  useUpstreamCredentials,
} from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";
import { Field, FieldLabel } from "@/components/ui/field";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

export function LogicalChannelsPage({ embedded = false }: { embedded?: boolean } = {}) {
  const [params, setParams] = useSearchParams();
  const groupId = params.get("group") ?? "all";
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
      embedded={embedded}
      title={t("Logical channels")}
      description={t(
        "Each channel binds one group, one upstream access, and at most one credential.",
      )}
      query={{
        data: query.data?.filter((channel) => groupId === "all" || channel.group_id === groupId),
        isLoading: query.isLoading || groups.isLoading || accesses.isLoading || credentials.isLoading,
        error: query.error ?? groups.error ?? accesses.error ?? credentials.error,
      }}
      rowKey={(channel) => channel.id}
      detailPath={(channel) => `/admin/routing/logical-channels/${channel.id}`}
      createLabel={t("New channel")}
      createPath="/admin/routing/logical-channels/new"
      headerActions={
        <Field orientation="horizontal" className="w-auto">
          <FieldLabel htmlFor="channel-group-filter">{t("Channel group")}</FieldLabel>
          <Select value={groupId} onValueChange={(value) => setParams((current) => {
            const next = new URLSearchParams(current);
            next.delete("page");
            if (value === "all") next.delete("group");
            else next.set("group", value);
            return next;
          })}>
            <SelectTrigger id="channel-group-filter"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="all">{t("All groups")}</SelectItem>
                {groups.data?.map((group) => <SelectItem key={group.id} value={group.id}>{group.name}</SelectItem>)}
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>
      }
      columns={[
        { key: "name", header: t("Name"), render: (channel) => channel.name },
        {
          key: "group",
          header: t("Group"),
          render: (channel) => groupNames.get(channel.group_id) ?? channel.group_id,
        },
        {
          key: "access",
          header: t("Upstream access"),
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
