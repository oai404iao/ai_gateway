import type {
  ChannelDeletionImpact,
  DeletionImpactNamedResource,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { apiFormatLabel } from "@/lib/permissions";

interface DeletionImpactSummaryProps {
  impact: ChannelDeletionImpact;
}

interface ImpactSectionProps {
  label: string;
  items: DeletionImpactNamedResource[];
}

function ImpactSection({ label, items }: ImpactSectionProps) {
  if (items.length === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      <dt className="font-medium">{label}</dt>
      <dd>
        <ul className="flex flex-col gap-1">
          {items.map((item) => (
            <li key={item.id} className="flex flex-wrap items-baseline gap-x-2">
              <span>{item.name}</span>
              <code className="text-muted-foreground text-[0.7rem]">
                {item.id}
              </code>
            </li>
          ))}
        </ul>
      </dd>
    </div>
  );
}

export function DeletionImpactSummary({
  impact,
}: DeletionImpactSummaryProps) {
  const { t } = useI18n();
  const dependentCount =
    impact.model_protocol_rules.length +
    impact.api_keys.length +
    impact.api_key_policies.length +
    impact.quota_visibility_user_groups.length;

  return (
    <div className="flex max-h-72 flex-col gap-3 overflow-y-auto text-sm">
      {dependentCount === 0 ? (
        <p>{t("No routing or authorization dependencies will be changed.")}</p>
      ) : null}
      <dl className="flex flex-col gap-3">
        <ImpactSection
          label={t("Channels to delete ({count})", {
            count: impact.channels.length,
          })}
          items={impact.channels}
        />
        {impact.model_protocol_rules.length > 0 ? (
          <div className="flex flex-col gap-1">
            <dt className="font-medium">
              {t("Protocol rules affected ({count})", {
                count: impact.model_protocol_rules.length,
              })}
            </dt>
            <dd>
              <ul className="flex flex-col gap-1">
                {impact.model_protocol_rules.map((rule) => (
                  <li key={rule.id} className="flex flex-col gap-0.5">
                    <div className="flex flex-wrap items-center gap-1">
                      <span className="font-mono text-xs">{rule.client_model}</span>
                      <span>· {apiFormatLabel(rule.api_format)}</span>
                      {rule.will_disable ? (
                        <span>· {t("will be disabled")}</span>
                      ) : null}
                    </div>
                    <div className="text-muted-foreground flex flex-wrap gap-x-3 text-xs">
                      {rule.removed_channel_group_ids.length > 0 ? (
                        <span>
                          {t("Group targets removed: {count}", {
                            count: rule.removed_channel_group_ids.length,
                          })}
                        </span>
                      ) : null}
                      {rule.removed_channel_ids.length > 0 ? (
                        <span>
                          {t("Channel routes removed: {count}", {
                            count: rule.removed_channel_ids.length,
                          })}
                        </span>
                      ) : null}
                      {rule.removed_tier_priorities.length > 0 ? (
                        <span>
                          {t("Tiers removed: {priorities}", {
                            priorities: rule.removed_tier_priorities.join(", "),
                          })}
                        </span>
                      ) : null}
                    </div>
                  </li>
                ))}
              </ul>
            </dd>
          </div>
        ) : null}
        <ImpactSection
          label={t("API keys to unbind ({count})", {
            count: impact.api_keys.length,
          })}
          items={impact.api_keys}
        />
        <ImpactSection
          label={t("API key policies to unbind ({count})", {
            count: impact.api_key_policies.length,
          })}
          items={impact.api_key_policies}
        />
        <ImpactSection
          label={t("Quota visibility assignments to remove ({count})", {
            count: impact.quota_visibility_user_groups.length,
          })}
          items={impact.quota_visibility_user_groups}
        />
      </dl>
    </div>
  );
}
