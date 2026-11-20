import { useEffect, useEffectEvent, useId, useMemo, useState } from "react";
import type {
  ApiOperation, ChannelCapabilityView, ConnectorPluginView, LogicalChannelView, OnboardingOperationInput,
  OperationRuleView, RoutingGroupView, UpstreamAccessView, UpstreamCredentialView,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Combobox, ComboboxCollection, ComboboxContent, ComboboxEmpty, ComboboxGroup,
  ComboboxInput, ComboboxItem, ComboboxList,
} from "@/components/ui/combobox";
import {
  Field, FieldDescription, FieldGroup, FieldLabel, FieldLegend, FieldSet,
} from "@/components/ui/field";
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import {
  useChannelCapabilities, useLogicalChannels, useRoutingGroups, useUpstreamAccesses,
  useUpstreamCredentials,
} from "@/features/admin/api";
import { API_OPERATIONS, apiOperationLabel, selectionStrategyLabel } from "@/lib/permissions";
import { useConnectorPlugins } from "@/features/admin/connectors/api";

interface Props {
  modelName: string;
  allowRegistration: boolean;
  rules: OperationRuleView[];
  onChange: (operations: OnboardingOperationInput[]) => void;
  onEdit?: () => void;
}

interface Topology {
  capabilities: ChannelCapabilityView[];
  channels: LogicalChannelView[];
  accesses: UpstreamAccessView[];
  groups: RoutingGroupView[];
  credentials: UpstreamCredentialView[];
  connectors: ConnectorPluginView[];
}

interface Row {
  capability: ChannelCapabilityView;
  channel?: string;
  access?: string;
  unavailable: string | null;
  model: string;
  priority: number;
  selected: boolean;
}

interface Section {
  operation: ApiOperation;
  rule?: OperationRuleView;
  rows: Row[];
}

function existingCandidate(rule: OperationRuleView | undefined, id: string, model: string, priority: number) {
  return rule?.routing_tiers.find((tier) => tier.priority === priority)?.candidates.find(
    (candidate) => candidate.capability_id === id && candidate.upstream_model === model,
  );
}

function initialSections({ modelName, allowRegistration, rules, capabilities, channels, accesses, groups, credentials, connectors }: Props & Topology): Section[] {
  const byId = <T extends { id: string },>(items: T[]) => new Map(items.map((item) => [item.id, item]));
  const channelById = byId(channels), accessById = byId(accesses), groupById = byId(groups), credentialById = byId(credentials);
  const connectorById = byId(connectors);
  return API_OPERATIONS.flatMap((operation) => {
    const rule = rules.find((item) => item.operation === operation);
    const tiers = [...(rule?.routing_tiers ?? [])].sort((a, b) => a.priority - b.priority);
    const rows = capabilities.filter((cap) => !cap.deleted_at && cap.settings.operation === operation).map((capability) => {
      const channel = channelById.get(capability.channel_id);
      const access = channel && accessById.get(channel.access_id);
      const group = channel && groupById.get(channel.group_id);
      const unavailable = !channel?.enabled || channel.deleted_at ? "Channel unavailable"
        : !group?.enabled || group.deleted_at ? "Channel group unavailable"
        : !access?.enabled || access.deleted_at ? "Upstream access unavailable"
        : !connectorById.get(access.connector_kind)?.operations.includes(operation) ? "Connector operation unavailable"
        : channel.credential_id && !credentialById.get(channel.credential_id)?.enabled ? "Credential unavailable"
        : !capability.settings.enabled ? "Capability disabled"
        : capability.auto_disabled ? "Automatically disabled" : null;
      const exact = capability.settings.available_models.includes(modelName);
      const alreadyRouted = tiers.find((tier) => tier.candidates.some(
        (candidate) => candidate.capability_id === capability.id && candidate.upstream_model === modelName,
      ));
      return {
        capability, channel: channel?.name, access: access?.name, unavailable,
        model: exact || allowRegistration ? modelName : "",
        priority: alreadyRouted?.priority ?? tiers[0]?.priority ?? 0,
        selected: exact && !unavailable && !alreadyRouted && rule?.enabled !== false,
      };
    }).sort((a, b) => (a.access ?? "").localeCompare(b.access ?? "") || (a.channel ?? "").localeCompare(b.channel ?? ""));
    return rows.length ? [{ operation, rule, rows }] : [];
  });
}

function operationInputs(sections: Section[]): OnboardingOperationInput[] {
  return sections.flatMap(({ operation, rule, rows }) => {
    const candidates = rows.filter((row) => row.selected).map(({ capability, model, priority }) => ({
      capability_id: capability.id, updated_at: capability.updated_at, upstream_model: model, priority,
      register_model: !capability.settings.available_models.includes(model),
    }));
    return candidates.length ? [{ operation, rule_updated_at: rule?.updated_at ?? null, candidates }] : [];
  });
}

export function OnboardingCandidates(props: Props) {
  if (!props.modelName.trim()) return null;
  return <TopologyLoader key={JSON.stringify([props.modelName, props.allowRegistration])} {...props} />;
}

function TopologyLoader(props: Props) {
  const capabilities = useChannelCapabilities(), channels = useLogicalChannels(), accesses = useUpstreamAccesses();
  const groups = useRoutingGroups(), credentials = useUpstreamCredentials(), connectors = useConnectorPlugins();
  if (!capabilities.data || !channels.data || !accesses.data || !groups.data || !credentials.data || !connectors.data) {
    const queries = [capabilities, channels, accesses, groups, credentials, connectors];
    return <AsyncResource isLoading={queries.some((q) => q.isLoading)} error={queries.find((q) => q.error)?.error}>{null}</AsyncResource>;
  }
  return <CandidateEditor {...props} capabilities={capabilities.data} channels={channels.data}
    accesses={accesses.data} groups={groups.data} credentials={credentials.data} connectors={connectors.data} />;
}

function CandidateEditor(props: Props & Topology) {
  const { t } = useI18n();
  // Refetches must not replace the versions or catalogue the administrator actually reviewed.
  const [sections, setSections] = useState(() => initialSections(props));
  const operations = useMemo(() => operationInputs(sections), [sections]);
  const notify = useEffectEvent(props.onChange);
  useEffect(() => { notify(operations); }, [operations]);

  function change(section: Section, row: Row, change: Partial<Pick<Row, "model" | "priority" | "selected">>) {
    const next = { ...row, ...change };
    if (!next.model || next.unavailable || existingCandidate(section.rule, next.capability.id, next.model, next.priority)) next.selected = false;
    if (row.selected === next.selected && row.priority === next.priority && row.model === next.model) return;
    props.onEdit?.();
    setSections((previous) => previous.map((s) => s.operation !== section.operation ? s : {
      ...s, rows: s.rows.map((item) => item.capability.id === row.capability.id ? next : item),
    }));
  }

  if (!sections.length) return <EmptyState title={t("No channel capabilities")} description={t("Configure a channel capability before adding routing candidates.")} />;
  return (
    <div className="flex min-w-0 flex-col gap-5">
      {sections.map((section) => {
        const { operation, rule, rows } = section;
        const saved = rule?.routing_tiers.map((tier) => tier.priority) ?? [];
        const priorities = [...new Set([...(saved.length ? saved : [0]), ...rows.map((row) => row.priority)])].sort((a, b) => a - b);
        const fallback = priorities[priorities.length - 1] + 1;
        if (fallback <= 2_147_483_647) priorities.push(fallback);
        const count = rows.filter((row) => row.selected).length;
        return (
          <FieldSet key={operation} className="min-w-0 gap-2">
            <FieldLegend>{apiOperationLabel(operation)}</FieldLegend>
            <FieldDescription>
              {rule ? (rule.enabled ? t("Rule stays enabled") : t("Rule stays disabled")) : (count ? t("Create enabled rule") : t("No rule yet"))}
              {" · "}{t("{count} to add", { count })}
            </FieldDescription>
            <ul className="@container flex min-w-0 flex-col gap-2">
              {rows.map((row) => <CandidateRow key={row.capability.id} {...{ row, section, priorities }}
                modelName={props.modelName} allowRegistration={props.allowRegistration} onChange={(patch) => change(section, row, patch)} />)}
            </ul>
          </FieldSet>
        );
      })}
    </div>
  );
}

function CandidateRow({ row, section, priorities, modelName, allowRegistration, onChange }: {
  row: Row; section: Section; priorities: number[]; modelName: string; allowRegistration: boolean;
  onChange: (patch: Partial<Pick<Row, "model" | "priority" | "selected">>) => void;
}) {
  const { t } = useI18n();
  const id = useId();
  const { capability, model, priority, unavailable } = row;
  const existing = existingCandidate(section.rule, capability.id, model, priority);
  const advertised = capability.settings.available_models;
  const missingExact = !advertised.includes(modelName);
  const register = missingExact && model === modelName;
  const options = allowRegistration && missingExact ? [modelName, ...advertised] : advertised;
  const channel = row.channel ?? t("Channel unavailable");
  const access = row.access ?? t("Upstream access unavailable");
  const name = `${access} / ${channel} · ${apiOperationLabel(section.operation)}`;
  const disabled = Boolean(unavailable || existing || !model);
  const status = unavailable ? t(unavailable) : existing ? t("Already in this tier · weight {weight}", { weight: existing.weight })
    : !model ? t("Exact name not registered") : register ? t("Not registered") : t("Registered");
  const action = register ? t("Register and route {model}", { model: modelName }) : t("Add route");
  return (
    <li className="grid min-w-0 items-start gap-3 rounded-md border p-3 @min-[38rem]:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)_minmax(0,1fr)]">
      <div className="flex min-w-0 flex-col gap-1 break-words">
        <span className="text-sm font-medium">{access}</span>
        <span className="text-sm">{channel}</span>
        <span className="text-xs text-muted-foreground" id={`${id}-status`}>{status}</span>
      </div>
      <FieldGroup className="min-w-0 gap-2">
        <Field data-disabled={Boolean(unavailable)}>
          <FieldLabel htmlFor={`${id}-model`}>{t("Upstream model")}</FieldLabel>
          <Combobox items={options} value={model || null} disabled={Boolean(unavailable) || !options.length}
            onValueChange={(value) => { if (value !== model) onChange({ model: value ?? "", selected: false }); }}>
            <ComboboxInput id={`${id}-model`} disabled={Boolean(unavailable) || !options.length} aria-label={t("Upstream model for {name}", { name })} placeholder={t("Choose an upstream model")} />
            <ComboboxContent>
              <ComboboxEmpty>{t("No matching models")}</ComboboxEmpty>
              <ComboboxList><ComboboxGroup><ComboboxCollection>
                {(value: string) => <ComboboxItem key={value} value={value}>
                  {value === modelName && missingExact ? t("{model} (not registered)", { model: value }) : value}
                </ComboboxItem>}
              </ComboboxCollection></ComboboxGroup></ComboboxList>
            </ComboboxContent>
          </Combobox>
        </Field>
        <Field orientation="horizontal" data-disabled={disabled}>
          <Checkbox id={`${id}-select`} checked={row.selected} disabled={disabled}
            aria-label={`${action}: ${name}`} aria-describedby={`${id}-status`}
            onCheckedChange={(selected) => onChange({ selected })} />
          <FieldLabel className="min-w-0 wrap-anywhere" htmlFor={`${id}-select`}>{action}</FieldLabel>
        </Field>
      </FieldGroup>
      <Field data-disabled={Boolean(unavailable)} className="min-w-0">
        <FieldLabel htmlFor={`${id}-tier`}>{t("Routing tier")}</FieldLabel>
        <Select value={String(priority)} disabled={Boolean(unavailable)} onValueChange={(value) => onChange({ priority: Number(value) })}>
          <SelectTrigger id={`${id}-tier`} className="w-full" aria-label={t("Routing tier for {name}", { name })}><SelectValue /></SelectTrigger>
          <SelectContent><SelectGroup>
            {priorities.map((p) => {
              const tier = section.rule?.routing_tiers.find((item) => item.priority === p);
              return <SelectItem key={p} value={String(p)}>{tier
                ? t("Priority {p} · {strategy}", { p, strategy: selectionStrategyLabel(tier.selection_strategy) })
                : t("Priority {p} · new tier", { p })}</SelectItem>;
            })}
          </SelectGroup></SelectContent>
        </Select>
        {row.selected && <FieldDescription>{t("New candidate · weight 1")}</FieldDescription>}
      </Field>
    </li>
  );
}
