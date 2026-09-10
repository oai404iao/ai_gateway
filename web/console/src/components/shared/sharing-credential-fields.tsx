import { useId, useMemo } from "react";
import type { SelfApiKeySharingCredentialOption } from "@/api/types";
import { Badge } from "@/components/ui/badge";
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
import { apiFormatLabel } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { useI18n } from "@/app/i18n";

interface SharingCredentialFieldsProps {
  credentials: SelfApiKeySharingCredentialOption[];
  selectedChannelIds: string[];
  onChange: (channelIds: string[]) => void;
  error?: string;
  className?: string;
}

export function SharingCredentialFields({
  credentials,
  selectedChannelIds,
  onChange,
  error,
  className,
}: SharingCredentialFieldsProps) {
  const { t } = useI18n();
  const idPrefix = useId();
  const selected = useMemo(() => new Set(selectedChannelIds), [selectedChannelIds]);

  const toggleCredential = (
    credential: SelfApiKeySharingCredentialOption,
    checked: boolean,
  ) => {
    const credentialChannels = new Set(credential.channel_ids);
    const next = selectedChannelIds.filter((id) => !credentialChannels.has(id));
    if (checked) next.push(...credential.channel_ids);
    onChange([...new Set(next)]);
  };

  return (
    <FieldSet
      className={cn("h-full rounded-lg border p-4", className)}
      data-invalid={Boolean(error) || undefined}
    >
      <FieldLegend>{t("Sharing credentials")}</FieldLegend>
      <FieldDescription>
        {t("Your fixed seats grant these credentials independently of API Key Policy.")}
      </FieldDescription>
      {credentials.length > 0 ? (
        <FieldGroup data-slot="checkbox-group" className="gap-4">
          {credentials.map((credential) => {
            const selectedCount = credential.channel_ids.filter((id) => selected.has(id)).length;
            const checked = selectedCount === credential.channel_ids.length;
            const partiallySelected = selectedCount > 0 && !checked;
            const inputId = `${idPrefix}-${credential.credential_id}`;
            return (
              <Field
                key={credential.credential_id}
                orientation="horizontal"
                data-invalid={Boolean(error) || undefined}
              >
                <Checkbox
                  id={inputId}
                  checked={checked}
                  indeterminate={partiallySelected}
                  aria-invalid={Boolean(error)}
                  aria-label={credential.name}
                  onCheckedChange={(nextChecked) =>
                    toggleCredential(credential, Boolean(nextChecked))
                  }
                />
                <FieldContent>
                  <FieldLabel htmlFor={inputId} className="font-normal">
                    <span className="flex flex-wrap items-center gap-2">
                      <span>{credential.name}</span>
                      {!credential.enabled ? <StatusBadge value={false} /> : null}
                      {partiallySelected ? (
                        <Badge variant="warning">{t("Partially selected")}</Badge>
                      ) : null}
                    </span>
                  </FieldLabel>
                  <FieldDescription className="flex flex-wrap gap-1">
                    {credential.api_formats.map((format) => (
                      <Badge key={format} variant="outline">
                        {apiFormatLabel(format)}
                      </Badge>
                    ))}
                  </FieldDescription>
                </FieldContent>
              </Field>
            );
          })}
        </FieldGroup>
      ) : (
        <FieldDescription>{t("No sharing credentials assigned.")}</FieldDescription>
      )}
      {error ? <FieldError>{error}</FieldError> : null}
    </FieldSet>
  );
}
