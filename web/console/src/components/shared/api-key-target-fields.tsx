import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { StatusBadge } from "@/components/shared/status-badge";
import type { ApiFormat } from "@/api/types";
import { apiFormatLabel } from "@/lib/permissions";
import { useI18n } from "@/app/i18n";

export interface ApiKeyTargetGroup {
  id: string;
  name: string;
  api_format: ApiFormat;
  enabled: boolean;
}

export interface ApiKeyTargetChannel {
  id: string;
  channel_group_id: string;
  channel_group_name?: string;
  name: string;
  api_format: ApiFormat;
  enabled: boolean;
  auto_disabled: boolean;
}

interface ApiKeyTargetFieldsProps {
  groups: ApiKeyTargetGroup[];
  channels: ApiKeyTargetChannel[];
  selectedGroupIds: string[];
  selectedChannelIds: string[];
  onChange: (groupIds: string[], channelIds: string[]) => void;
  error?: string;
}

export function ApiKeyTargetFields({
  groups,
  channels,
  selectedGroupIds,
  selectedChannelIds,
  onChange,
  error,
}: ApiKeyTargetFieldsProps) {
  const { t } = useI18n();
  const groupNames = new Map(groups.map((group) => [group.id, group.name]));

  const toggleGroup = (groupId: string, checked: boolean) => {
    const nextGroups = checked
      ? [...selectedGroupIds, groupId]
      : selectedGroupIds.filter((id) => id !== groupId);
    const nextChannels = checked
      ? selectedChannelIds.filter(
          (channelId) =>
            channels.find((channel) => channel.id === channelId)?.channel_group_id !== groupId,
        )
      : selectedChannelIds;
    onChange(nextGroups, nextChannels);
  };

  const toggleChannel = (channelId: string, checked: boolean) => {
    onChange(
      selectedGroupIds,
      checked
        ? [...selectedChannelIds, channelId]
        : selectedChannelIds.filter((id) => id !== channelId),
    );
  };

  return (
    <>
      <FieldSet>
        <FieldLegend variant="label">
          {t("Channel groups ({count})", { count: groups.length })}
        </FieldLegend>
        <FieldDescription>
          {t("Selecting a group allows the key to use every channel in that group.")}
        </FieldDescription>
        <FieldGroup data-slot="checkbox-group" className="gap-3">
          {groups.map((group) => {
            const inputId = `api_key_group_${group.id}`;
            return (
              <Field key={group.id} orientation="horizontal" data-invalid={Boolean(error)}>
                <Checkbox
                  id={inputId}
                  checked={selectedGroupIds.includes(group.id)}
                  aria-label={`${group.name} (${apiFormatLabel(group.api_format)})`}
                  aria-invalid={Boolean(error)}
                  onCheckedChange={(checked) => toggleGroup(group.id, Boolean(checked))}
                />
                <FieldLabel htmlFor={inputId} className="font-normal">
                  <span className="flex flex-wrap items-center gap-2">
                    <span>{group.name}</span>
                    <StatusBadge
                      value={group.api_format}
                      label={apiFormatLabel(group.api_format)}
                      variant="info"
                    />
                    {!group.enabled ? <StatusBadge value={false} /> : null}
                  </span>
                </FieldLabel>
              </Field>
            );
          })}
          {groups.length === 0 ? (
            <FieldDescription>{t("No selectable channel groups.")}</FieldDescription>
          ) : null}
        </FieldGroup>
      </FieldSet>
      <FieldSet>
        <FieldLegend variant="label">
          {t("Individual channels ({count})", { count: channels.length })}
        </FieldLegend>
        <FieldDescription>
          {t("Choose individual channels when the whole group should not be available.")}
        </FieldDescription>
        <FieldGroup data-slot="checkbox-group" className="gap-3">
          {channels.map((channel) => {
            const coveredByGroup = selectedGroupIds.includes(channel.channel_group_id);
            const inputId = `api_key_channel_${channel.id}`;
            return (
              <Field
                key={channel.id}
                orientation="horizontal"
                data-disabled={coveredByGroup || undefined}
                data-invalid={Boolean(error)}
              >
                <Checkbox
                  id={inputId}
                  checked={selectedChannelIds.includes(channel.id)}
                  aria-label={`${channel.name} (${
                    channel.channel_group_name ??
                    groupNames.get(channel.channel_group_id) ??
                    channel.channel_group_id
                  })`}
                  disabled={coveredByGroup}
                  aria-invalid={Boolean(error)}
                  onCheckedChange={(checked) => toggleChannel(channel.id, Boolean(checked))}
                />
                <FieldLabel htmlFor={inputId} className="font-normal">
                  <span className="flex flex-wrap items-center gap-2">
                    <span>
                      {channel.name} ·{" "}
                      {channel.channel_group_name ??
                        groupNames.get(channel.channel_group_id) ??
                        channel.channel_group_id}
                    </span>
                    <StatusBadge
                      value={channel.api_format}
                      label={apiFormatLabel(channel.api_format)}
                      variant="info"
                    />
                    {!channel.enabled || channel.auto_disabled ? (
                      <StatusBadge value={false} />
                    ) : null}
                  </span>
                </FieldLabel>
              </Field>
            );
          })}
          {channels.length === 0 ? (
            <FieldDescription>{t("No selectable individual channels.")}</FieldDescription>
          ) : null}
        </FieldGroup>
      </FieldSet>
      {error ? <FieldError className="md:col-span-2">{error}</FieldError> : null}
    </>
  );
}
