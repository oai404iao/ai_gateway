import { useId, useMemo } from "react";
import type { SelfApiKeySharingChannelOption } from "@/api/types";
import { Checkbox } from "@/components/ui/checkbox";
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
import { StatusBadge } from "@/components/shared/status-badge";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

interface SharingChannelFieldsProps {
  channels: SelfApiKeySharingChannelOption[];
  selectedChannelIds: string[];
  onChange: (channelIds: string[]) => void;
  error?: string;
  className?: string;
}

export function SharingChannelFields({
  channels,
  selectedChannelIds,
  onChange,
  error,
  className,
}: SharingChannelFieldsProps) {
  const { t } = useI18n();
  const idPrefix = useId();
  const selected = useMemo(() => new Set(selectedChannelIds), [selectedChannelIds]);

  const toggleChannel = (
    channel: SelfApiKeySharingChannelOption,
    checked: boolean,
  ) => {
    const next = selectedChannelIds.filter((id) => id !== channel.channel_id);
    if (checked) next.push(channel.channel_id);
    onChange([...new Set(next)]);
  };

  return (
    <FieldSet
      className={cn("h-full rounded-lg border p-4", className)}
      data-invalid={Boolean(error) || undefined}
    >
      <FieldLegend>{t("Sharing channels")}</FieldLegend>
      <FieldDescription>
        {t("Your fixed seats grant these logical channels independently of API Key Policy.")}
      </FieldDescription>
      {channels.length > 0 ? (
        <FieldGroup data-slot="checkbox-group" className="gap-4">
          {channels.map((channel) => {
            const checked = selected.has(channel.channel_id);
            const inputId = `${idPrefix}-${channel.channel_id}`;
            return (
              <Field
                key={channel.channel_id}
                orientation="horizontal"
                data-invalid={Boolean(error) || undefined}
              >
                <Checkbox
                  id={inputId}
                  checked={checked}
                  aria-invalid={Boolean(error)}
                  aria-label={channel.channel_name}
                  onCheckedChange={(nextChecked) =>
                    toggleChannel(channel, Boolean(nextChecked))
                  }
                />
                <FieldContent>
                  <FieldLabel htmlFor={inputId} className="font-normal">
                    <span className="flex flex-wrap items-center gap-2">
                      <span>{channel.channel_name} ({channel.name})</span>
                      {!channel.enabled ? <StatusBadge value={false} /> : null}
                    </span>
                  </FieldLabel>
                </FieldContent>
              </Field>
            );
          })}
        </FieldGroup>
      ) : (
        <FieldDescription>{t("No sharing channels assigned.")}</FieldDescription>
      )}
      {error ? <FieldError>{error}</FieldError> : null}
    </FieldSet>
  );
}
