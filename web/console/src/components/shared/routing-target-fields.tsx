import { useId, useMemo, useState } from "react";
import { ChevronDown, Search, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { StatusBadge } from "@/components/shared/status-badge";
import type { ApiFormat } from "@/api/types";
import { API_FORMATS, apiFormatLabel } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

export interface RoutingTargetGroup {
  id: string;
  name: string;
  api_format: ApiFormat;
  enabled: boolean;
  priority?: number;
}

export interface RoutingTargetChannel {
  id: string;
  channel_group_id: string;
  channel_group_name?: string;
  channel_group_enabled: boolean;
  name: string;
  api_format: ApiFormat;
  enabled: boolean;
  auto_disabled: boolean;
}

interface RoutingTargetFieldsProps {
  groups: RoutingTargetGroup[];
  channels: RoutingTargetChannel[];
  selectedGroupIds: string[];
  selectedChannelIds: string[];
  onChange: (groupIds: string[], channelIds: string[]) => void;
  error?: string;
  className?: string;
}

const FORMAT_ORDER: Record<ApiFormat, number> = {
  open_ai_chat_completions: 0,
  open_ai_responses: 1,
  open_ai_images: 2,
};

function compareNames(left: string, right: string): number {
  return left.localeCompare(right, undefined, { sensitivity: "base" });
}

function matchesSearch(values: Array<string | undefined>, search: string): boolean {
  return values.some((value) => value?.toLocaleLowerCase().includes(search));
}

export function RoutingTargetFields({
  groups,
  channels,
  selectedGroupIds,
  selectedChannelIds,
  onChange,
  error,
  className,
}: RoutingTargetFieldsProps) {
  const { t } = useI18n();
  const idPrefix = useId();
  const [search, setSearch] = useState("");
  const [showDisabled, setShowDisabled] = useState(false);
  const [channelsOpen, setChannelsOpen] = useState(false);
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const selectedGroupSet = useMemo(() => new Set(selectedGroupIds), [selectedGroupIds]);
  const selectedChannelSet = useMemo(
    () => new Set(selectedChannelIds),
    [selectedChannelIds],
  );
  const groupById = useMemo(
    () => new Map(groups.map((group) => [group.id, group])),
    [groups],
  );

  const channelAvailable = (channel: RoutingTargetChannel) =>
    channel.enabled &&
    !channel.auto_disabled &&
    channel.channel_group_enabled;

  const sortedGroups = useMemo(
    () =>
      [...groups].sort(
        (left, right) =>
          FORMAT_ORDER[left.api_format] - FORMAT_ORDER[right.api_format] ||
          (left.priority ?? Number.MAX_SAFE_INTEGER) -
            (right.priority ?? Number.MAX_SAFE_INTEGER) ||
          compareNames(left.name, right.name) ||
          compareNames(left.id, right.id),
      ),
    [groups],
  );
  const sortedChannels = useMemo(
    () =>
      [...channels].sort((left, right) => {
        const leftGroup = groupById.get(left.channel_group_id);
        const rightGroup = groupById.get(right.channel_group_id);
        return (
          FORMAT_ORDER[left.api_format] - FORMAT_ORDER[right.api_format] ||
          (leftGroup?.priority ?? Number.MAX_SAFE_INTEGER) -
            (rightGroup?.priority ?? Number.MAX_SAFE_INTEGER) ||
          compareNames(
            left.channel_group_name ?? leftGroup?.name ?? left.channel_group_id,
            right.channel_group_name ?? rightGroup?.name ?? right.channel_group_id,
          ) ||
          compareNames(left.name, right.name) ||
          compareNames(left.id, right.id)
        );
      }),
    [channels, groupById],
  );

  const visibleGroups = sortedGroups.filter(
    (group) =>
      (showDisabled || group.enabled || selectedGroupSet.has(group.id)) &&
      (!normalizedSearch ||
        matchesSearch(
          [group.name, apiFormatLabel(group.api_format), String(group.priority ?? "")],
          normalizedSearch,
        )),
  );
  const visibleChannels = sortedChannels.filter((channel) => {
    const group = groupById.get(channel.channel_group_id);
    return (
      (showDisabled || channelAvailable(channel) || selectedChannelSet.has(channel.id)) &&
      (!normalizedSearch ||
        matchesSearch(
          [
            channel.name,
            channel.channel_group_name,
            group?.name,
            apiFormatLabel(channel.api_format),
          ],
          normalizedSearch,
        ))
    );
  });
  const groupCategories = API_FORMATS.map((apiFormat) => ({
    apiFormat,
    items: visibleGroups.filter((group) => group.api_format === apiFormat),
  })).filter((category) => category.items.length > 0);
  const channelCategories = API_FORMATS.map((apiFormat) => ({
    apiFormat,
    items: visibleChannels.filter((channel) => channel.api_format === apiFormat),
  })).filter((category) => category.items.length > 0);
  const disabledTargetCount =
    groups.filter((group) => !group.enabled).length +
    channels.filter((channel) => !channelAvailable(channel)).length;

  const toggleGroup = (groupId: string, checked: boolean) => {
    const nextGroups = checked
      ? [...new Set([...selectedGroupIds, groupId])]
      : selectedGroupIds.filter((id) => id !== groupId);
    const nextChannels = checked
      ? selectedChannelIds.filter(
          (channelId) =>
            channels.find((channel) => channel.id === channelId)?.channel_group_id !==
            groupId,
        )
      : selectedChannelIds;
    onChange(nextGroups, nextChannels);
  };

  const toggleChannel = (channelId: string, checked: boolean) => {
    onChange(
      selectedGroupIds,
      checked
        ? [...new Set([...selectedChannelIds, channelId])]
        : selectedChannelIds.filter((id) => id !== channelId),
    );
  };

  return (
    <FieldSet
      className={cn("rounded-lg border p-4", className)}
      data-invalid={Boolean(error) || undefined}
    >
      <FieldLegend>{t("Routing targets")}</FieldLegend>
      <FieldDescription>
        {t("Targets are grouped by API format and sorted by priority and name.")}
      </FieldDescription>

      <div className="flex flex-wrap gap-2">
        <Badge variant="secondary">
          {t("{count} groups selected", { count: selectedGroupIds.length })}
        </Badge>
        <Badge variant="secondary">
          {t("{count} individual channels selected", {
            count: selectedChannelIds.length,
          })}
        </Badge>
      </div>

      <Field>
        <FieldLabel htmlFor={`${idPrefix}-search`} className="sr-only">
          {t("Search routing targets")}
        </FieldLabel>
        <InputGroup>
          <InputGroupAddon>
            <Search aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            id={`${idPrefix}-search`}
            type="search"
            value={search}
            aria-label={t("Search routing targets")}
            placeholder={t("Search channel groups or channels")}
            onChange={(event) => setSearch(event.target.value)}
          />
          {search ? (
            <InputGroupAddon align="inline-end">
              <InputGroupButton
                size="icon-xs"
                aria-label={t("Clear search")}
                onClick={() => setSearch("")}
              >
                <X data-icon="inline-start" />
              </InputGroupButton>
            </InputGroupAddon>
          ) : null}
        </InputGroup>
      </Field>
      <Field
        orientation="horizontal"
        data-disabled={disabledTargetCount === 0 || undefined}
      >
        <Checkbox
          id={`${idPrefix}-show-disabled`}
          checked={showDisabled}
          disabled={disabledTargetCount === 0}
          onCheckedChange={(checked) => setShowDisabled(Boolean(checked))}
        />
        <FieldLabel htmlFor={`${idPrefix}-show-disabled`} className="font-normal">
          {t("Show disabled targets ({count})", { count: disabledTargetCount })}
        </FieldLabel>
      </Field>

      <FieldSet>
        <FieldLegend variant="label">
          {t("Channel groups ({count})", { count: visibleGroups.length })}
        </FieldLegend>
        <FieldDescription>
          {t("Selecting a group applies to every channel in that group.")}
        </FieldDescription>
        {groupCategories.length > 0 ? (
          <FieldGroup className="gap-5">
            {groupCategories.map((category) => (
              <FieldSet key={category.apiFormat}>
                <FieldLegend variant="label">
                  <span className="flex items-center gap-2">
                    <span>{apiFormatLabel(category.apiFormat)}</span>
                    <Badge variant="outline">{category.items.length}</Badge>
                  </span>
                </FieldLegend>
                <FieldGroup data-slot="checkbox-group" className="gap-3">
                  {category.items.map((group) => {
                    const checked = selectedGroupSet.has(group.id);
                    const disabled = !group.enabled && !checked;
                    const inputId = `${idPrefix}-group-${group.id}`;
                    return (
                      <Field
                        key={group.id}
                        orientation="horizontal"
                        data-disabled={disabled || undefined}
                        data-invalid={Boolean(error) || undefined}
                      >
                        <Checkbox
                          id={inputId}
                          checked={checked}
                          disabled={disabled}
                          aria-label={`${group.name} (${apiFormatLabel(group.api_format)})`}
                          aria-invalid={Boolean(error)}
                          onCheckedChange={(nextChecked) =>
                            toggleGroup(group.id, Boolean(nextChecked))
                          }
                        />
                        <FieldContent>
                          <FieldLabel htmlFor={inputId} className="font-normal">
                            <span className="flex flex-wrap items-center gap-2">
                              <span>{group.name}</span>
                              {group.priority !== undefined ? (
                                <Badge variant="outline">
                                  {t("Priority")}: {group.priority}
                                </Badge>
                              ) : null}
                              {!group.enabled ? <StatusBadge value={false} /> : null}
                            </span>
                          </FieldLabel>
                        </FieldContent>
                      </Field>
                    );
                  })}
                </FieldGroup>
              </FieldSet>
            ))}
          </FieldGroup>
        ) : (
          <FieldDescription>
            {normalizedSearch
              ? t("No channel groups match the current filters.")
              : t("No selectable channel groups.")}
          </FieldDescription>
        )}
      </FieldSet>

      <Collapsible open={channelsOpen} onOpenChange={setChannelsOpen}>
        <Card size="sm">
          <CardHeader>
            <CardTitle>{t("Advanced: individual channels")}</CardTitle>
            <CardDescription>
              {t("Use individual channels only when the whole group should not be selected.")}
            </CardDescription>
            <CardAction>
              <CollapsibleTrigger render={<Button type="button" variant="outline" size="sm" />}>
                {channelsOpen
                  ? t("Hide individual channels")
                  : t("Show individual channels ({count})", {
                      count: visibleChannels.length,
                    })}
                <ChevronDown
                  data-icon="inline-end"
                  className={cn("transition-transform", channelsOpen && "rotate-180")}
                />
              </CollapsibleTrigger>
            </CardAction>
          </CardHeader>
          <CollapsibleContent>
            <CardContent>
              {channelCategories.length > 0 ? (
                <FieldGroup className="gap-5">
                  {channelCategories.map((category) => (
                    <FieldSet key={category.apiFormat}>
                      <FieldLegend variant="label">
                        <span className="flex items-center gap-2">
                          <span>{apiFormatLabel(category.apiFormat)}</span>
                          <Badge variant="outline">{category.items.length}</Badge>
                        </span>
                      </FieldLegend>
                      <FieldGroup data-slot="checkbox-group" className="gap-3">
                        {category.items.map((channel) => {
                          const group = groupById.get(channel.channel_group_id);
                          const checked = selectedChannelSet.has(channel.id);
                          const coveredByGroup = selectedGroupSet.has(
                            channel.channel_group_id,
                          );
                          const available = channelAvailable(channel);
                          const disabled =
                            (coveredByGroup && !checked) || (!available && !checked);
                          const inputId = `${idPrefix}-channel-${channel.id}`;
                          const groupName =
                            channel.channel_group_name ??
                            group?.name ??
                            channel.channel_group_id;
                          return (
                            <Field
                              key={channel.id}
                              orientation="horizontal"
                              data-disabled={disabled || undefined}
                              data-invalid={Boolean(error) || undefined}
                            >
                              <Checkbox
                                id={inputId}
                                checked={checked}
                                disabled={disabled}
                                aria-label={`${channel.name} (${groupName})`}
                                aria-invalid={Boolean(error)}
                                onCheckedChange={(nextChecked) =>
                                  toggleChannel(channel.id, Boolean(nextChecked))
                                }
                              />
                              <FieldContent>
                                <FieldLabel htmlFor={inputId} className="font-normal">
                                  <span className="flex flex-wrap items-center gap-2">
                                    <span>{channel.name}</span>
                                    {!available ? <StatusBadge value={false} /> : null}
                                    {channel.auto_disabled ? (
                                      <Badge variant="destructive">
                                        {t("auto-disabled")}
                                      </Badge>
                                    ) : null}
                                  </span>
                                </FieldLabel>
                                <FieldDescription>{groupName}</FieldDescription>
                              </FieldContent>
                            </Field>
                          );
                        })}
                      </FieldGroup>
                    </FieldSet>
                  ))}
                </FieldGroup>
              ) : (
                <FieldDescription>
                  {normalizedSearch
                    ? t("No individual channels match the current filters.")
                    : t("No selectable individual channels.")}
                </FieldDescription>
              )}
            </CardContent>
          </CollapsibleContent>
        </Card>
      </Collapsible>

      {error ? <FieldError>{error}</FieldError> : null}
    </FieldSet>
  );
}
