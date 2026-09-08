import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useSearchParams } from "react-router";
import {
  ArrowLeft,
  ArrowRight,
  Boxes,
  Calculator,
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
  List,
  Network,
  Pencil,
  Plus,
  Route,
  Search,
  Workflow,
} from "lucide-react";
import { useI18n } from "@/app/i18n";
import type {
  ChannelGroupView,
  ControlPlaneModel,
  ModelRuleView,
} from "@/api/types";
import {
  useChannelGroups,
  useChannels,
  useModelRules,
  useModels,
} from "@/features/admin/api";
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
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
} from "@/components/ui/input-group";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Separator } from "@/components/ui/separator";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { apiFormatLabel, selectionStrategyLabel } from "@/lib/permissions";
import { formatDecimal } from "@/lib/formatters";
import { cn } from "@/lib/utils";
import { adminPath, MODEL_SETUP_PATH } from "./model-setup-navigation";
import {
  configurationPath,
  LENS_PATHS,
  modelNeedsAttention,
  rulesForGroup,
  supplyEntries,
  supplyNeedsAttention,
  targetChannels,
  type ConfigurationData,
  type ConfigurationLens,
  type SupplyEntry,
} from "./configuration-graph";

const LENSES = [
  {
    key: "routes",
    label: "Client routes",
    description: "What clients can call",
    icon: Route,
  },
  {
    key: "supply",
    label: "Channel supply",
    description: "Where requests are sent",
    icon: Network,
  },
  {
    key: "models",
    label: "Models & pricing",
    description: "What upstream models cost",
    icon: Boxes,
  },
] as const;
const FORMATS = [
  "open_ai_chat_completions",
  "open_ai_responses",
  "open_ai_images",
] as const;
const PAGE_SIZE = 24;

function ActionLink({
  to,
  children,
  primary = false,
}: {
  to: string;
  children: ReactNode;
  primary?: boolean;
}) {
  return (
    <Button
      nativeButton={false}
      role="link"
      size="sm"
      variant={primary ? "default" : "outline"}
      render={<Link to={to} />}
    >
      {children}
    </Button>
  );
}

export function ConfigurationNav({ lens }: { lens?: ConfigurationLens }) {
  const { t } = useI18n();
  return (
    <nav
      aria-label={t("Configuration views")}
      className="grid grid-cols-3 gap-2"
    >
      {LENSES.map(({ key, label, description, icon: Icon }) => (
        <Link
          key={key}
          to={LENS_PATHS[key]}
          aria-current={lens === key ? "page" : undefined}
          className={cn(
            "flex min-w-0 items-center gap-3 rounded-xl border px-3 py-3 transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring sm:px-4",
            lens === key ? "border-primary bg-accent" : "bg-card",
          )}
        >
          <Icon
            className="hidden size-5 shrink-0 sm:block"
            aria-hidden="true"
          />
          <span className="flex min-w-0 flex-1 flex-col text-center sm:text-left">
            <span className="text-sm font-medium">{t(label)}</span>
            <span className="hidden text-xs text-muted-foreground sm:block">
              {t(description)}
            </span>
          </span>
          {lens === key && (
            <Check
              className="hidden size-4 shrink-0 sm:block"
              aria-hidden="true"
            />
          )}
        </Link>
      ))}
    </nav>
  );
}

/** Dense tables remain an explicit tool, not the default configuration journey. */
export function ConfigurationTableView({
  lens,
  children,
}: {
  lens: ConfigurationLens;
  children: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <ConfigurationNav lens={lens} />
      <div>
        <ActionLink to={LENS_PATHS[lens]}>
          <ArrowLeft data-icon="inline-start" />
          {t("Back to workbench")}
        </ActionLink>
      </div>
      {children}
    </div>
  );
}

function Facts({ items }: { items: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-2 gap-x-5 gap-y-4">
      {items.map(([label, value]) => (
        <div key={label} className="flex min-w-0 flex-col gap-1">
          <dt className="text-xs text-muted-foreground">{label}</dt>
          <dd className="break-words text-sm font-medium">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function Relation({
  to,
  title,
  description,
  badge,
}: {
  to: string;
  title: string;
  description?: string;
  badge?: ReactNode;
}) {
  return (
    <Link
      to={to}
      className="flex min-w-0 items-center gap-3 rounded-lg border px-3 py-3 transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
    >
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="break-words text-sm font-medium">{title}</span>
        {description && (
          <span className="break-words text-xs text-muted-foreground">
            {description}
          </span>
        )}
      </span>
      {badge}
      <ArrowRight
        className="size-4 shrink-0 text-muted-foreground"
        aria-hidden="true"
      />
    </Link>
  );
}

function Section({
  title,
  description,
  children,
  actions,
}: {
  title: string;
  description?: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <Card size="sm" className="min-w-0">
      <CardHeader>
        <CardTitle role="heading" aria-level={3}>
          {title}
        </CardTitle>
        {description && (
          <CardDescription className="col-span-full">
            {description}
          </CardDescription>
        )}
        {actions && <CardAction className="row-span-1">{actions}</CardAction>}
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-3">
        {children}
      </CardContent>
    </Card>
  );
}

function InspectorHeader({
  eyebrow,
  title,
  description,
  children,
  actions,
}: {
  eyebrow: string;
  title: string;
  description?: string | null;
  children: ReactNode;
  actions: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <p className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
            {eyebrow}
          </p>
          <h2 className="break-words text-2xl font-semibold tracking-tight">
            {title}
          </h2>
          <div className="flex flex-wrap gap-2">{children}</div>
        </div>
        <div className="flex flex-wrap gap-2">{actions}</div>
      </div>
      {description && (
        <p className="break-words text-sm text-muted-foreground">
          {description}
        </p>
      )}
    </div>
  );
}

function RuleInspector({
  rule,
  data,
  returnTo,
}: {
  rule: ModelRuleView;
  data: ConfigurationData;
  returnTo: string;
}) {
  const { t } = useI18n();
  const model = data.models.find((item) => item.id === rule.upstream_model_id);
  const channels = targetChannels(rule, data.channels);
  const editPath = adminPath(`/admin/routing/model-rules/${rule.id}`, {
    returnTo,
  });
  return (
    <>
      <InspectorHeader
        eyebrow={t("Client-facing route")}
        title={rule.client_model}
        description={rule.description}
        actions={
          <ActionLink to={editPath} primary>
            <Pencil data-icon="inline-start" />
            {t("Edit route")}
          </ActionLink>
        }
      >
        <Badge variant="outline">{apiFormatLabel(rule.api_format)}</Badge>
        <StatusBadge value={rule.routing_status} />
        <StatusBadge value={rule.enabled} />
      </InspectorHeader>
      <Section
        title={t("Request path")}
        description={t(
          "Client model → priced upstream model → ordered channel tiers. No conversion between API formats.",
        )}
      >
        <div className="grid items-center gap-3 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]">
          <div className="flex min-w-0 flex-col gap-1 rounded-lg bg-muted p-3">
            <span className="text-xs text-muted-foreground">
              {t("Client model")}
            </span>
            <span className="break-words font-mono text-sm">
              {rule.client_model}
            </span>
            <span className="text-xs text-muted-foreground">
              {apiFormatLabel(rule.api_format)}
            </span>
          </div>
          <ArrowRight
            className="hidden size-4 text-muted-foreground sm:block"
            aria-hidden="true"
          />
          {model ? (
            <Relation
              to={configurationPath("models", model.id)}
              title={model.display_name}
              description={model.source_model_id}
            />
          ) : (
            <Alert variant="destructive">
              <AlertTitle>{t("Upstream model unavailable")}</AlertTitle>
              <AlertDescription>{rule.upstream_model}</AlertDescription>
            </Alert>
          )}
        </div>
        <Separator />
        <Facts
          items={[
            [t("Target channels"), rule.target_channel_count],
            [t("Model-capable channels"), rule.model_capable_channel_count],
            [t("Active channels"), rule.active_channel_count],
            [t("Routing tiers"), rule.routing_tiers.length],
          ]}
        />
      </Section>
      {rule.routing_status !== "ready" && (
        <Alert>
          <AlertTitle>{t("Review this route")}</AlertTitle>
          <AlertDescription>
            {t(
              "Check the route switch, upstream model, channel-group switches and model capabilities. Status is reported by the gateway, not inferred from this diagram.",
            )}
          </AlertDescription>
        </Alert>
      )}
      <Section
        title={t("Routing plan")}
        description={t(
          "Lower priority numbers run first. Strategy and channel weights belong to each route tier.",
        )}
        actions={
          <ActionLink to={editPath}>{t("Configure targets")}</ActionLink>
        }
      >
        {[...rule.routing_tiers]
          .sort((a, b) => a.priority - b.priority)
          .map((tier) => (
            <div
              key={tier.priority}
              className="flex flex-col gap-3 rounded-lg border p-3"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <Badge variant="secondary">
                  {t("Priority {priority}", { priority: tier.priority })}
                </Badge>
                <span className="text-xs text-muted-foreground">
                  {selectionStrategyLabel(tier.selection_strategy)}
                </span>
              </div>
              {tier.channel_groups.map((target) => {
                const group = data.groups.find(
                  (item) => item.id === target.channel_group_id,
                );
                const members = channels.filter(
                  (channel) =>
                    channel.channel_group_id === target.channel_group_id &&
                    (target.channel_selection === "all" ||
                      target.channels.some(
                        (item) => item.channel_id === channel.id,
                      )),
                );
                return (
                  <div
                    key={target.channel_group_id}
                    className="flex flex-col gap-2"
                  >
                    <Relation
                      to={configurationPath("supply", target.channel_group_id)}
                      title={group?.name ?? t("Unavailable channel group")}
                      description={
                        target.channel_selection === "all"
                          ? t("All channels · default weight {weight}", {
                              weight: target.default_weight ?? "—",
                            })
                          : t("Selected channels only")
                      }
                      badge={group && <StatusBadge value={group.enabled} />}
                    />
                    <ul className="flex flex-col gap-2 pl-3 text-xs">
                      {members.map((channel) => (
                        <li
                          key={channel.id}
                          className="flex flex-wrap items-center justify-between gap-2"
                        >
                          <span className="break-words">{channel.name}</span>
                          <span className="text-muted-foreground">
                            {t("Weight {weight}", {
                              weight:
                                target.channels.find(
                                  (item) => item.channel_id === channel.id,
                                )?.weight ??
                                target.default_weight ??
                                "—",
                            })}
                            {" · "}
                            {channel.available_models.includes(
                              rule.upstream_model,
                            )
                              ? t("Model supported")
                              : t("Model not supported")}
                          </span>
                        </li>
                      ))}
                      {members.length === 0 && (
                        <li className="text-muted-foreground">
                          {t(
                            "No target channels in the current configuration.",
                          )}
                        </li>
                      )}
                    </ul>
                  </div>
                );
              })}
            </div>
          ))}
        {rule.routing_tiers.length === 0 && (
          <EmptyState
            title={t("No routing targets")}
            description={t("Edit the route to add a channel group.")}
          />
        )}
      </Section>
    </>
  );
}

function GroupPanel({
  group,
  data,
  returnTo,
}: {
  group: ChannelGroupView;
  data: ConfigurationData;
  returnTo: string;
}) {
  const { t } = useI18n();
  const [params] = useSearchParams();
  const query = (params.get("q") ?? "").trim().toLowerCase();
  const [channelSearch, setChannelSearch] = useState("");
  const [channelPage, setChannelPage] = useState(1);
  const channels = data.channels.filter(
    (channel) => channel.channel_group_id === group.id,
  );
  useEffect(() => {
    setChannelSearch(group.name.toLowerCase().includes(query) ? "" : query);
    setChannelPage(1);
  }, [query, group.name]);
  const matchingChannels = channels.filter((channel) =>
    `${channel.name} ${channel.available_models.join(" ")}`
      .toLowerCase()
      .includes(channelSearch.trim().toLowerCase()),
  );
  const channelPages = Math.max(1, Math.ceil(matchingChannels.length / 8));
  const currentPage = Math.min(channelPage, channelPages);
  const rules = rulesForGroup(data.rules, group.id);
  const managed = group.connector_kind === "codex_oauth";
  const canonical =
    (group.connector_pool_id
      ? data.groups.find(
          (item) =>
            item.connector_pool_id === group.connector_pool_id &&
            item.connector_kind === "codex_oauth" &&
            item.api_format === "open_ai_responses",
        )
      : undefined) ?? group;
  return (
    <Section
      title={group.name}
      description={apiFormatLabel(group.api_format)}
      actions={
        <ActionLink
          to={adminPath(`/admin/routing/channel-groups/${group.id}`, {
            returnTo,
          })}
        >
          <Pencil data-icon="inline-start" />
          {t("Edit group")}
        </ActionLink>
      }
    >
      <div className="flex flex-wrap gap-2">
        <StatusBadge value={group.enabled} />
        <Badge variant="secondary">
          {t("{count} channels", { count: channels.length })}
        </Badge>
        <Badge variant="outline">
          {t("{count} routes", { count: rules.length })}
        </Badge>
      </div>
      {!group.enabled && (
        <Alert>
          <AlertTitle>{t("Group disabled")}</AlertTitle>
          <AlertDescription>
            {t(
              "Every channel in this group is excluded, even if its own switch is on.",
            )}
          </AlertDescription>
        </Alert>
      )}
      {(channels.length > 8 || channelSearch) && (
        <InputGroup>
          <InputGroupAddon>
            <Search aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            type="search"
            aria-label={t("Search channels in {name}", { name: group.name })}
            placeholder={t("Search names, models…")}
            value={channelSearch}
            onChange={(event) => {
              setChannelSearch(event.target.value);
              setChannelPage(1);
            }}
          />
        </InputGroup>
      )}
      {matchingChannels
        .slice((currentPage - 1) * 8, currentPage * 8)
        .map((channel) => (
          <div
            key={channel.id}
            className="flex min-w-0 flex-col gap-3 rounded-lg border p-3"
          >
            <Relation
              title={channel.name}
              description={
                managed
                  ? t("Managed credential projection")
                  : t("Billing multiplier: {value}", {
                      value: formatDecimal(channel.billing_multiplier),
                    })
              }
              to={adminPath(
                managed
                  ? `/admin/providers/codex-oauth/${canonical.id}`
                  : `/admin/routing/channels/${channel.id}`,
                { returnTo },
              )}
              badge={
                <StatusBadge
                  value={
                    channel.auto_disabled ? "auto_disabled" : channel.enabled
                  }
                />
              }
            />
            <div className="flex flex-wrap gap-1">
              {channel.available_models.slice(0, 8).map((name) => {
                const model = data.models.find(
                  (item) => item.source_model_id === name,
                );
                return model ? (
                  <Link
                    key={name}
                    to={configurationPath("models", model.id)}
                    className="max-w-full rounded-md underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                  >
                    <Badge variant="outline" className="max-w-full">
                      <span className="truncate">{name}</span>
                    </Badge>
                  </Link>
                ) : (
                  <Badge key={name} variant="secondary" className="max-w-full">
                    <span className="truncate">{name}</span>
                  </Badge>
                );
              })}
              {channel.available_models.length > 8 && (
                <Badge variant="secondary">
                  +{channel.available_models.length - 8}
                </Badge>
              )}
              {channel.available_models.length === 0 && (
                <span className="text-xs text-muted-foreground">
                  {t("No model capabilities configured")}
                </span>
              )}
            </div>
            <div className="flex flex-wrap gap-2">
              <ActionLink
                to={adminPath("/admin/routing/model-rules/new", {
                  channelGroupId: group.id,
                  channelId: channel.id,
                  apiFormat: group.api_format,
                  returnTo,
                })}
              >
                <Plus data-icon="inline-start" />
                {t("Route this channel")}
              </ActionLink>
              {!managed && (
                <ActionLink
                  to={adminPath("/admin/routing/channels/new", {
                    copyFrom: channel.id,
                    channelGroupId: group.id,
                    returnTo,
                  })}
                >
                  <Copy data-icon="inline-start" />
                  {t("Copy")}
                </ActionLink>
              )}
            </div>
          </div>
        ))}
      {channelPages > 1 && (
        <div className="flex items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground">
            {currentPage}/{channelPages}
          </span>
          <div className="flex gap-1">
            <Button
              variant="outline"
              size="icon-sm"
              aria-label={t("Previous channels")}
              disabled={currentPage === 1}
              onClick={() => setChannelPage(currentPage - 1)}
            >
              <ChevronLeft />
            </Button>
            <Button
              variant="outline"
              size="icon-sm"
              aria-label={t("Next channels")}
              disabled={currentPage === channelPages}
              onClick={() => setChannelPage(currentPage + 1)}
            >
              <ChevronRight />
            </Button>
          </div>
        </div>
      )}
      {channels.length > 0 && matchingChannels.length === 0 && (
        <EmptyState
          title={t("No matching configuration")}
          description={t("Clear filters or create a new resource.")}
        />
      )}
      {channels.length === 0 && (
        <EmptyState
          title={t("No channels in this group")}
          description={t("Create a channel and assign it to this group.")}
        />
      )}
      {!managed && (
        <div>
          <ActionLink
            to={adminPath("/admin/routing/channels/new", {
              channelGroupId: group.id,
              returnTo,
            })}
          >
            <Plus data-icon="inline-start" />
            {t("New channel")}
          </ActionLink>
        </div>
      )}
      <Separator />
      <h3 className="text-sm font-medium">{t("Routes using this group")}</h3>
      {rules.map((rule) => (
        <Relation
          key={rule.id}
          to={configurationPath("routes", rule.id)}
          title={rule.client_model}
          description={apiFormatLabel(rule.api_format)}
          badge={<StatusBadge value={rule.routing_status} />}
        />
      ))}
      {rules.length === 0 && (
        <p className="text-sm text-muted-foreground">
          {t(
            "No routes use this group yet. Adding a channel does not publish a client model.",
          )}
        </p>
      )}
    </Section>
  );
}

function SupplyInspector({
  entry,
  data,
  returnTo,
}: {
  entry: SupplyEntry;
  data: ConfigurationData;
  returnTo: string;
}) {
  const { t } = useI18n();
  return (
    <>
      <InspectorHeader
        eyebrow={t(entry.managed ? "Shared credential pool" : "Channel group")}
        title={entry.name}
        description={t(
          entry.managed
            ? "Credentials are shared. Format switches, capabilities and routing remain independent."
            : "A supply group owns channels. Model rules decide priority, weights and which channels receive requests.",
        )}
        actions={
          entry.managed ? (
            <ActionLink
              primary
              to={adminPath(`/admin/providers/codex-oauth/${entry.id}`, {
                returnTo,
              })}
            >
              {t("Manage shared credentials")}
            </ActionLink>
          ) : null
        }
      >
        <Badge variant="outline">
          {entry.managed ? "Codex OAuth" : "OpenAI-compatible"}
        </Badge>
        {entry.groups.map((group) => (
          <Badge variant="secondary" key={group.id}>
            {apiFormatLabel(group.api_format)}
          </Badge>
        ))}
      </InspectorHeader>
      <div
        className={cn(
          "grid min-w-0 items-start gap-4",
          entry.groups.length > 1 && "2xl:grid-cols-2",
        )}
      >
        {entry.groups.map((group) => (
          <GroupPanel
            key={group.id}
            group={group}
            data={data}
            returnTo={returnTo}
          />
        ))}
      </div>
    </>
  );
}

function ModelInspector({
  model,
  data,
  returnTo,
}: {
  model: ControlPlaneModel;
  data: ConfigurationData;
  returnTo: string;
}) {
  const { t } = useI18n();
  const rules = data.rules.filter(
    (rule) => rule.upstream_model_id === model.id,
  );
  const groups = data.groups.filter((group) =>
    data.channels.some(
      (channel) =>
        channel.channel_group_id === group.id &&
        channel.available_models.includes(model.source_model_id),
    ),
  );
  return (
    <>
      <InspectorHeader
        eyebrow={t("Priced upstream model")}
        title={model.display_name}
        description={model.source_model_id}
        actions={
          <ActionLink
            to={adminPath(`/admin/models/${model.id}`, { returnTo })}
            primary
          >
            <Pencil data-icon="inline-start" />
            {t("Edit model")}
          </ActionLink>
        }
      >
        <Badge variant="outline">
          {model.provider_name || t("Unspecified provider")}
        </Badge>
        <StatusBadge value={model.enabled} />
      </InspectorHeader>
      <Section
        title={t("Pricing")}
        description={t(
          "USD per one million tokens. Channel multipliers and advanced billing may also apply.",
        )}
        actions={
          <ActionLink
            to={adminPath(`/admin/models/${model.id}/pricing`, { returnTo })}
          >
            <Calculator data-icon="inline-start" />
            {t("Configure pricing")}
          </ActionLink>
        }
      >
        <Facts
          items={[
            [t("Input price"), formatDecimal(model.input_unit_price)],
            [t("Output price"), formatDecimal(model.output_unit_price)],
            [
              t("Cache read price"),
              formatDecimal(model.cached_input_unit_price),
            ],
            [
              t("Cache write price"),
              formatDecimal(model.cache_write_unit_price),
            ],
          ]}
        />
        <div>
          <ActionLink
            to={adminPath("/admin/models/new", {
              copyFrom: model.id,
              returnTo,
            })}
          >
            <Copy data-icon="inline-start" />
            {t("Copy model")}
          </ActionLink>
        </div>
      </Section>
      <div className="grid min-w-0 items-start gap-4 2xl:grid-cols-2">
        <Section
          title={t("Supply supporting this model")}
          description={t(
            "Declared model capabilities, not a live health check.",
          )}
        >
          {groups.map((group) => (
            <Relation
              key={group.id}
              to={configurationPath("supply", group.id)}
              title={group.name}
              description={apiFormatLabel(group.api_format)}
              badge={<StatusBadge value={group.enabled} />}
            />
          ))}
          {groups.length === 0 && (
            <EmptyState
              title={t("No supporting channels")}
              description={t(
                "Add this upstream model ID to a channel's model capabilities.",
              )}
            />
          )}
          <ActionLink to={configurationPath("supply")}>
            {t("Browse channel supply")}
            <ArrowRight data-icon="inline-end" />
          </ActionLink>
        </Section>
        <Section
          title={t("Client routes")}
          description={t(
            "A price record alone does not expose a model to clients.",
          )}
        >
          {rules.map((rule) => (
            <Relation
              key={rule.id}
              to={configurationPath("routes", rule.id)}
              title={rule.client_model}
              description={apiFormatLabel(rule.api_format)}
              badge={<StatusBadge value={rule.routing_status} />}
            />
          ))}
          {rules.length === 0 && (
            <EmptyState
              title={t("Not used by any route")}
              description={t(
                "Create a route to connect this model to a client-facing name.",
              )}
            />
          )}
          <ActionLink
            primary
            to={adminPath("/admin/routing/model-rules/new", {
              upstreamModelId: model.id,
              returnTo,
            })}
          >
            <Plus data-icon="inline-start" />
            {t("Create route for model")}
          </ActionLink>
        </Section>
      </div>
    </>
  );
}

interface DirectoryEntry {
  id: string;
  title: string;
  subtitle: string;
  search: string;
  attention: boolean;
  facets: string[];
  status: ReactNode;
  rule?: ModelRuleView;
  supply?: SupplyEntry;
  model?: ControlPlaneModel;
}

export function ConfigurationWorkbench({
  lens = "routes",
}: {
  lens?: ConfigurationLens;
}) {
  const { t } = useI18n();
  const rules = useModelRules();
  const groups = useChannelGroups();
  const channels = useChannels();
  const models = useModels();
  const [params, setParams] = useSearchParams();
  const location = useLocation();
  const inspector = useRef<HTMLElement>(null);
  const [focusInspector, setFocusInspector] = useState(false);
  const search = params.get("q") ?? "";
  const facet = params.get("facet") ?? "all";
  const attention = params.get("state") === "attention";
  const selectedId = params.get("selected");
  const data = useMemo<ConfigurationData>(
    () => ({
      rules: rules.data ?? [],
      groups: groups.data ?? [],
      channels: channels.data ?? [],
      models: models.data ?? [],
    }),
    [rules.data, groups.data, channels.data, models.data],
  );
  const supplies = useMemo(() => supplyEntries(data.groups), [data.groups]);
  const entries = useMemo<DirectoryEntry[]>(() => {
    if (lens === "routes")
      return [...data.rules]
        .sort(
          (a, b) =>
            a.client_model.localeCompare(b.client_model) ||
            a.api_format.localeCompare(b.api_format),
        )
        .map((rule) => ({
          id: rule.id,
          title: rule.client_model,
          subtitle: `${apiFormatLabel(rule.api_format)} · ${rule.upstream_model}`,
          search: `${rule.client_model} ${rule.upstream_model} ${rule.description ?? ""}`,
          attention: rule.routing_status !== "ready",
          facets: [rule.api_format],
          status: <StatusBadge value={rule.routing_status} />,
          rule,
        }));
    if (lens === "supply")
      return supplies.map((supply) => ({
        id: supply.id,
        title: supply.name,
        subtitle: supply.managed
          ? t("Shared credential pool")
          : apiFormatLabel(supply.groups[0].api_format),
        search: `${supply.name} ${supply.groups.map((group) => group.name).join(" ")} ${data.channels
          .filter((channel) =>
            supply.groups.some(
              (group) => group.id === channel.channel_group_id,
            ),
          )
          .map(
            (channel) =>
              `${channel.name} ${channel.available_models.join(" ")}`,
          )
          .join(" ")}`,
        attention: supplyNeedsAttention(supply, data.channels),
        facets: supply.groups.map((group) => group.api_format),
        status: (
          <Badge
            variant={
              supplyNeedsAttention(supply, data.channels)
                ? "warning"
                : "secondary"
            }
          >
            {t("{count} channels", {
              count: data.channels.filter((channel) =>
                supply.groups.some(
                  (group) => group.id === channel.channel_group_id,
                ),
              ).length,
            })}
          </Badge>
        ),
        supply,
      }));
    return [...data.models]
      .sort((a, b) => a.display_name.localeCompare(b.display_name))
      .map((model) => ({
        id: model.id,
        title: model.display_name,
        subtitle: model.source_model_id,
        search: `${model.display_name} ${model.source_model_id} ${model.provider_name ?? ""}`,
        attention: modelNeedsAttention(model, data.rules),
        facets: [model.provider_name?.trim() || "__unspecified"],
        status: <StatusBadge value={model.enabled} />,
        model,
      }));
  }, [data, lens, supplies, t]);
  const filtered = entries.filter(
    (entry) =>
      (!attention || entry.attention) &&
      (facet === "all" || entry.facets.includes(facet)) &&
      entry.search.toLowerCase().includes(search.trim().toLowerCase()),
  );
  const requestedPage = Number(params.get("page") ?? "1");
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const page = Math.min(
    pageCount,
    Math.max(1, Number.isSafeInteger(requestedPage) ? requestedPage : 1),
  );
  const visible = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const selected = selectedId
    ? filtered.find(
        (entry) =>
          entry.id === selectedId ||
          entry.supply?.groups.some((group) => group.id === selectedId),
      )
    : visible[0];
  const returnTo = adminPath(location.pathname, {
    q: search,
    facet: facet === "all" ? null : facet,
    state: attention ? "attention" : null,
    page: page > 1 ? String(page) : null,
    selected: selected?.id,
  });
  const setFilter = (key: string, value: string) => {
    setParams(
      (current) => {
        const next = new URLSearchParams(current);
        if (value && value !== "all") next.set(key, value);
        else next.delete(key);
        next.delete("page");
        next.delete("selected");
        return next;
      },
      { replace: true },
    );
  };
  const createPath =
    lens === "routes"
      ? "/admin/routing/model-rules/new"
      : lens === "supply"
        ? "/admin/routing/channel-groups/new"
        : "/admin/models/new";
  const createLabel =
    lens === "routes"
      ? "New rule"
      : lens === "supply"
        ? "New group"
        : "New upstream model";
  useEffect(() => {
    if (focusInspector) {
      inspector.current?.focus({ preventScroll: true });
      setFocusInspector(false);
    }
  }, [focusInspector, selected?.id]);

  return (
    <div className="flex min-w-0 flex-col gap-5">
      <PageHeader
        title={t("Model configuration")}
        description={t(
          "Trace a client model to its upstream price and channel supply. Inspect relationships before changing configuration.",
        )}
        actions={
          <ActionLink to={MODEL_SETUP_PATH}>
            <Workflow data-icon="inline-start" />
            {t("Guided model setup")}
          </ActionLink>
        }
      />
      <ConfigurationNav lens={lens} />
      <AsyncResource
        isLoading={
          rules.isLoading ||
          groups.isLoading ||
          channels.isLoading ||
          models.isLoading
        }
        error={rules.error ?? groups.error ?? channels.error ?? models.error}
      >
        <div className="grid min-w-0 items-start gap-5 lg:grid-cols-[minmax(16rem,0.8fr)_minmax(0,2fr)]">
          <section
            aria-label={t("Configuration directory")}
            className={cn(
              "flex min-w-0 flex-col gap-3 lg:sticky lg:top-20",
              selectedId && "hidden lg:flex",
            )}
          >
            <div className="flex items-center justify-between gap-2">
              <h2 className="text-sm font-semibold">
                {t(LENSES.find((item) => item.key === lens)!.label)}{" "}
                <span className="text-muted-foreground">
                  / {entries.length}
                </span>
              </h2>
              <ActionLink primary to={adminPath(createPath, { returnTo })}>
                <Plus data-icon="inline-start" />
                {t(createLabel)}
              </ActionLink>
            </div>
            <InputGroup>
              <InputGroupAddon>
                <Search aria-hidden="true" />
              </InputGroupAddon>
              <InputGroupInput
                type="search"
                value={search}
                onChange={(event) => setFilter("q", event.target.value)}
                aria-label={t("Search configuration")}
                placeholder={t("Search names, models…")}
              />
            </InputGroup>
            <Select
              value={facet}
              onValueChange={(value) => setFilter("facet", value ?? "all")}
            >
              <SelectTrigger
                className="w-full"
                aria-label={t(
                  lens === "models" ? "Provider filter" : "API format filter",
                )}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value="all">
                    {t(lens === "models" ? "All providers" : "All API formats")}
                  </SelectItem>
                  {lens === "models"
                    ? [
                        ...new Set(
                          data.models.map(
                            (model) =>
                              model.provider_name?.trim() || "__unspecified",
                          ),
                        ),
                      ]
                        .sort()
                        .map((provider) => (
                          <SelectItem key={provider} value={provider}>
                            {provider === "__unspecified"
                              ? t("Unspecified provider")
                              : provider}
                          </SelectItem>
                        ))
                    : FORMATS.map((format) => (
                        <SelectItem key={format} value={format}>
                          {apiFormatLabel(format)}
                        </SelectItem>
                      ))}
                </SelectGroup>
              </SelectContent>
            </Select>
            <ToggleGroup
              value={[attention ? "attention" : "all"]}
              onValueChange={(values) => {
                if (values[0]) setFilter("state", values[0]);
              }}
              variant="outline"
              size="sm"
              spacing={0}
              aria-label={t("Configuration status")}
            >
              <ToggleGroupItem value="all">
                {t("All")} ({entries.length})
              </ToggleGroupItem>
              <ToggleGroupItem value="attention">
                {t("Needs attention")} (
                {entries.filter((entry) => entry.attention).length})
              </ToggleGroupItem>
            </ToggleGroup>
            <div className="flex min-w-0 flex-col overflow-hidden rounded-xl border bg-card">
              <div className="flex flex-col divide-y lg:max-h-[max(12rem,calc(100dvh-30rem))] lg:overflow-y-auto">
                {visible.map((entry) => (
                  <Link
                    key={entry.id}
                    to={adminPath(location.pathname, {
                      q: search,
                      facet: facet === "all" ? null : facet,
                      state: attention ? "attention" : null,
                      page: page > 1 ? String(page) : null,
                      selected: entry.id,
                    })}
                    aria-current={
                      selected?.id === entry.id ? "true" : undefined
                    }
                    onClick={() => setFocusInspector(true)}
                    className={cn(
                      "flex min-w-0 flex-col gap-2 border-l-2 p-3 transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
                      selected?.id === entry.id
                        ? "border-l-primary bg-accent"
                        : "border-l-transparent",
                    )}
                  >
                    <span className="flex items-start justify-between gap-2">
                      <span className="break-words text-sm font-medium">
                        {entry.title}
                      </span>
                      {entry.status}
                    </span>
                    <span
                      className="truncate text-xs text-muted-foreground"
                      title={entry.subtitle}
                    >
                      {entry.subtitle}
                    </span>
                  </Link>
                ))}
                {visible.length === 0 && (
                  <EmptyState
                    title={t("No matching configuration")}
                    description={t("Clear filters or create a new resource.")}
                  />
                )}
              </div>
              <div className="flex items-center justify-between gap-2 border-t px-3 py-2">
                <span className="text-xs text-muted-foreground">
                  {t("{count} results", { count: filtered.length })} · {page}/
                  {pageCount}
                </span>
                <div className="flex gap-1">
                  {([-1, 1] as const).map((delta) => (
                    <Button
                      key={delta}
                      variant="ghost"
                      size="icon-sm"
                      disabled={delta < 0 ? page <= 1 : page >= pageCount}
                      aria-label={t(delta < 0 ? "Previous page" : "Next page")}
                      onClick={() =>
                        setParams((current) => {
                          const next = new URLSearchParams(current);
                          next.set("page", String(page + delta));
                          next.delete("selected");
                          return next;
                        })
                      }
                    >
                      {delta < 0 ? <ChevronLeft /> : <ChevronRight />}
                    </Button>
                  ))}
                </div>
              </div>
            </div>
            {(search || facet !== "all" || attention) && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setParams({}, { replace: true })}
              >
                {t("Clear filters")}
              </Button>
            )}
            <ActionLink to={`${LENS_PATHS[lens]}?mode=table`}>
              <List data-icon="inline-start" />
              {t("Table & batch tools")}
            </ActionLink>
          </section>
          <section
            ref={inspector}
            tabIndex={-1}
            aria-label={t("Configuration inspector")}
            className={cn(
              "flex min-w-0 flex-col gap-4 rounded-xl outline-none",
              !selectedId && "hidden lg:flex",
            )}
          >
            <div className="lg:hidden">
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setParams((current) => {
                    const next = new URLSearchParams(current);
                    next.delete("selected");
                    return next;
                  });
                }}
              >
                <ArrowLeft data-icon="inline-start" />
                {t("Back to directory")}
              </Button>
            </div>
            {selected?.rule && (
              <RuleInspector
                rule={selected.rule}
                data={data}
                returnTo={returnTo}
              />
            )}
            {selected?.supply && (
              <SupplyInspector
                entry={selected.supply}
                data={data}
                returnTo={returnTo}
              />
            )}
            {selected?.model && (
              <ModelInspector
                model={selected.model}
                data={data}
                returnTo={returnTo}
              />
            )}
            {!selected && (
              <EmptyState
                title={t(
                  selectedId
                    ? "Selection unavailable"
                    : "Select a configuration",
                )}
                description={t(
                  "Choose a result from the directory, or clear filters to see all resources.",
                )}
                actions={
                  <Button variant="outline" onClick={() => setParams({})}>
                    {t("Clear filters")}
                  </Button>
                }
              />
            )}
          </section>
        </div>
      </AsyncResource>
    </div>
  );
}
