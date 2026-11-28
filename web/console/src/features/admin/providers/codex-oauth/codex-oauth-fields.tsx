import { useI18n } from "@/app/i18n";
import { formatEstimatedQuotaTotal, formatUsd } from "@/lib/formatters";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { formatDateTime } from "@/lib/dates";
import { NO_PROXY, type CredentialSettingsState } from "./codex-oauth-helpers";

export function QuotaWindow({
  label,
  percent,
  resetAt,
  costAmount,
}: {
  label: string;
  percent: number | null;
  resetAt: string | null;
  costAmount: string | null;
}) {
  const { t } = useI18n();
  return (
    <div className="min-w-40 space-y-1">
      <div className="flex justify-between gap-3 text-xs">
        <span>{t(label)}</span>
        <span>{percent === null ? "—" : `${percent}%`}</span>
      </div>
      <Progress value={percent ?? 0} />
      {resetAt ? (
        <p className="text-xs text-muted-foreground">
          {t("Resets {time}", { time: formatDateTime(resetAt) })}
        </p>
      ) : null}
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">{t("Period spend")}</span>
        <span className="font-medium tabular-nums">
          {formatUsd(costAmount)}
        </span>
      </div>
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">
          {t("Estimated total quota")}
        </span>
        <span className="font-medium tabular-nums">
          {formatEstimatedQuotaTotal(costAmount, percent)}
        </span>
      </div>
    </div>
  );
}

export function ProxyField({
  value,
  onChange,
  proxies,
}: {
  value: string;
  onChange: (value: string) => void;
  proxies: Array<{ id: string; name: string }>;
}) {
  const { t } = useI18n();
  return (
    <Field>
      <FieldLabel>{t("Outbound proxy")}</FieldLabel>
      <Select
        value={value || NO_PROXY}
        onValueChange={(next) => onChange(next === NO_PROXY ? "" : next)}
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem value={NO_PROXY}>{t("Direct")}</SelectItem>
            {proxies.map((proxy) => (
              <SelectItem key={proxy.id} value={proxy.id}>
                {proxy.name}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
    </Field>
  );
}

export function SettingsFields({
  state,
  onChange,
  proxies,
}: {
  state: CredentialSettingsState;
  onChange: (patch: Partial<CredentialSettingsState>) => void;
  proxies: Array<{ id: string; name: string }>;
}) {
  const { t } = useI18n();
  return (
    <FieldGroup>
      <Field>
        <FieldLabel htmlFor="codex-label">{t("Label")}</FieldLabel>
        <Input
          id="codex-label"
          value={state.label}
          onChange={(event) => onChange({ label: event.target.value })}
          placeholder={t("Personal Codex account")}
        />
      </Field>
      <ProxyField
        value={state.proxy_id}
        onChange={(proxy_id) => onChange({ proxy_id })}
        proxies={proxies}
      />
      <Field>
        <FieldLabel htmlFor="codex-threshold">
          {t("Quota threshold (%)")}
        </FieldLabel>
        <Input
          id="codex-threshold"
          type="number"
          min={1}
          max={100}
          value={state.quota_threshold_percent}
          onChange={(event) =>
            onChange({ quota_threshold_percent: event.target.value })
          }
        />
        <FieldDescription>
          {t(
            "New requests stop using this credential at the threshold; existing sticky Responses sessions may continue.",
          )}
        </FieldDescription>
      </Field>
    </FieldGroup>
  );
}
