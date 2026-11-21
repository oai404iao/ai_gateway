import { useId, useLayoutEffect, useRef, useState } from "react";
import { useLocation, useNavigate, useOutletContext, useSearchParams } from "react-router";
import { ArrowLeft, ArrowRight, Calculator, Check, Copy, Plus, Settings2 } from "lucide-react";
import { useI18n } from "@/app/i18n";
import { AsyncResource, ErrorAlert } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { NavigationLink } from "@/components/shared/navigation-link";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { useModels, useOperationRules, useRoutingProfiles } from "@/features/admin/api";
import { CatalogPage } from "@/features/admin/catalog/catalog-page";
import { OperationRuleDetailPage } from "@/features/admin/routing/operation-rules/operation-rule-detail-page";
import { API_OPERATIONS, apiOperationLabel } from "@/lib/permissions";
import { groupModelsByProvider } from "./model-groups";
import type { ModelDirectoryContext } from "./model-workspace-layout";
import { adminPath } from "@/features/admin/model-setup/model-setup-navigation";
import { formatCompactTokens, formatTokens } from "@/lib/formatters";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";
import type { ControlPlaneModel } from "@/api/types";
import styles from "./models-page.module.css";

const priceColumns = [
  ["Input price", "input_unit_price"],
  ["Cache hit price", "cached_input_unit_price"],
  ["Output price", "output_unit_price"],
] as const;

type DirectoryHistory = { listKey: string; steps: number };

function ModelPrices({ model, compact = false }: { model: ControlPlaneModel; compact?: boolean }) {
  const { t } = useI18n();
  const unit = model.price_unit_tokens;
  const divisor = unit >= 1e9 ? 1e9 : unit >= 1e6 ? 1e6 : 1e3;
  const tokens = unit >= 1e3 && unit % (divisor / 100) !== 0
    ? formatTokens(unit) : formatCompactTokens(unit);
  return (
    <span className="flex w-full flex-col gap-1 text-xs">
      <span>{compact ? t("{currency} / {tokens} tokens", { currency: "USD", tokens }) : t(
        "Base prices · {currency} / {tokens} tokens", { currency: "USD", tokens },
      )}</span>
      <span className="grid grid-cols-3 gap-2">
        {priceColumns.map(([label, field]) => (
          <span key={field} className="flex min-w-0 flex-col gap-1">
            <span className={cn(compact && "sr-only")}>{t(label)}</span>
            <span className="wrap-anywhere tabular-nums">{model[field]}</span>
          </span>
        ))}
      </span>
    </span>
  );
}

export function ModelsPage() {
  const { t } = useI18n();
  const origin = usePageOrigin();
  const { key: locationKey, state: locationState } = useLocation();
  const navigate = useNavigate();
  const history = (locationState as { directory?: DirectoryHistory } | null)?.directory;
  const [params, setParams] = useSearchParams();
  const models = useModels();
  const profiles = useRoutingProfiles();
  const rules = useOperationRules();
  const hasLists = Boolean(models.data && profiles.data && rules.data);
  const listError = models.error ?? profiles.error ?? rules.error;
  const view = params.get("view") === "prices" ? "prices" : "models";
  const search = params.get("q") ?? "";
  const query = search.trim().toLowerCase();
  const groups = groupModelsByProvider(models.data ?? [], t("Unspecified provider"))
    .map(({ provider, models }) => ({
      provider,
      models: provider.toLowerCase().includes(query) ? models : models.filter((model) =>
        [model.display_name, model.source_model_id].some((value) => value.toLowerCase().includes(query)),
      ),
    }))
    .filter((group) => group.models.length > 0);
  const visibleIds = groups.flatMap((group) => group.models.map((model) => model.id));
  const modelId = params.get("model");
  const selected = models.data?.find((model) => model.id === modelId);
  const profile = profiles.data?.find((item) => item.model_id === selected?.id);
  const modelRules = (rules.data ?? []).filter((rule) =>
    profile && rule.model_routing_profile_id === profile.id,
  );
  const ruleId = params.get("rule");
  const selectedRule = modelRules.find((rule) => rule.id === ruleId);
  const adding = ruleId === "new" && modelRules.length < API_OPERATIONS.length;
  const heading = useRef<HTMLHeadingElement>(null);
  const detail = useRef<HTMLElement>(null);
  const back = useRef<HTMLButtonElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const idPrefix = useId();
  const lastModel = useRef<string | null>(null);
  const position = useOutletContext<ModelDirectoryContext>();
  const [focusId, setFocusId] = useState<string | null>(null);
  const [missingRow, setMissingRow] = useState(false);
  const tabStop = [focusId, modelId, visibleIds[0]].find((id) => id && visibleIds.includes(id));

  useLayoutEffect(() => {
    if (!hasLists || view !== "models") return;
    const previous = lastModel.current;
    lastModel.current = modelId;
    if (modelId) {
      if (modelId === previous) return;
      if (detail.current) detail.current.scrollTop = 0;
      if (back.current?.getClientRects().length) {
        window.scrollTo(0, 0);
        heading.current?.focus({ preventScroll: true });
      }
    } else if (previous || position.current?.listKey === locationKey) {
      const saved = position.current;
      const returnToRow = saved && (saved.listKey === locationKey ||
        (saved.modelId === previous && saved.search === search));
      const row = returnToRow && document.getElementById(`${idPrefix}-${saved.modelId}`);
      if (row && saved) {
        window.scrollTo(0, saved.scrollY);
        row.focus({ preventScroll: true });
        const bounds = row.getBoundingClientRect();
        const top = parseFloat(getComputedStyle(row).scrollMarginTop);
        if (bounds.top < top || bounds.bottom > window.innerHeight) row.scrollIntoView({ block: "nearest" });
      } else {
        searchInput.current?.focus();
        setMissingRow(Boolean(returnToRow));
      }
    }
  }, [hasLists, idPrefix, locationKey, modelId, position, search, view]);

  const updateParams = (values: Record<string, string | null>, replace = false) => {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(values)) {
      if (value === null) next.delete(key);
      else next.set(key, value);
    }
    if (next.toString() === params.toString()) return;
    const directory = next.has("model") ? (
      modelId ? history && { ...history, steps: history.steps + (replace ? 0 : 1) } :
        { listKey: locationKey, steps: 1 }
    ) : undefined;
    setParams(next, { replace, preventScrollReset: true, state: directory ? { directory } : null });
  };
  const setSearch = (q: string) => {
    setMissingRow(false);
    updateParams({ q: q || null }, true);
  };
  const focusDetail = () => {
    if (detail.current) detail.current.scrollTop = 0;
    heading.current?.focus({ preventScroll: true });
  };
  const returnToDirectory = () => {
    const saved = position.current;
    if (history && saved?.listKey === history.listKey && saved.search === search) {
      void navigate(-history.steps);
    } else {
      updateParams({ model: null, rule: null }, true);
    }
  };

  return (
    <div className={cn("flex min-w-0 flex-col gap-6", view === "models" && styles.clientView)}>
      <PageHeader
        title={t("Model configuration")}
        actions={
          <>
            <NavigationLink to={withReturnTo("/admin/models/new", origin)} variant="outline">{t("New client model")}</NavigationLink>
            <NavigationLink to="/admin/models/onboarding" variant="default" size="default">
              <Plus data-icon="inline-start" />{t("Onboard model")}
            </NavigationLink>
          </>
        }
      />
      <Tabs value={view} className="min-h-0 flex-1"
        onValueChange={(value) => updateParams({ view: String(value) })}>
        <TabsList className="shrink-0">
          <TabsTrigger value="models">{t("Client models")}</TabsTrigger>
          <TabsTrigger value="prices">{t("Price sync")}</TabsTrigger>
        </TabsList>
        <TabsContent value="prices"><CatalogPage embedded /></TabsContent>
        <TabsContent value="models" className="flex min-h-0 flex-col gap-3">
          {hasLists && listError && <ErrorAlert error={listError} />}
          <AsyncResource
            isLoading={models.isLoading || profiles.isLoading || rules.isLoading}
            error={hasLists ? null : listError}
          >
            <div className={styles.workspace}>
              <Card variant="section" className={cn("min-h-0 min-w-0", styles.directory)} data-detail-open={modelId ? "" : undefined}>
                <CardHeader>
                  <CardTitle>{t("Client models")}</CardTitle>
                  <Input ref={searchInput} type="search" value={search}
                    aria-label={t("Search name, model ID, or provider")}
                    placeholder={t("Search name, model ID, or provider")}
                    onChange={(event) => setSearch(event.target.value)} />
                  <div className="flex flex-wrap items-center justify-between gap-x-2 text-xs text-muted-foreground">
                    <span role="status">{missingRow ? t("The previous model is no longer in the results.") :
                      t("Models found: {count}", { count: visibleIds.length })}</span>
                    {search && <Button variant="ghost" size="xs" onClick={() => {
                      setSearch("");
                      searchInput.current?.focus();
                    }}>{t("Clear search")}</Button>}
                  </div>
                  {visibleIds.length > 0 && <div aria-hidden="true" className="grid grid-cols-3 gap-2 border-t px-2 pt-2 text-xs text-muted-foreground">
                    {priceColumns.map(([label]) => <span key={label}>{t(label)}</span>)}
                  </div>}
                </CardHeader>
                <CardContent className={cn("flex flex-col gap-3", styles.results)}>
                  {!models.data?.length ? <EmptyState title={t("No client models yet")} /> :
                    visibleIds.length === 0 && <EmptyState title={t("No matching models")} />}
                  <p id={`${idPrefix}-keyboard`} className="sr-only">{t("Use the arrow keys to move between models.")}</p>
                  {groups.map((group) => (
                    <section key={group.provider} className="flex flex-col gap-1">
                      <h2 className="wrap-anywhere px-2 text-sm font-medium text-muted-foreground">{group.provider}</h2>
                      {group.models.map((model) => {
                        const current = selected?.id === model.id;
                        const rowId = `${idPrefix}-${model.id}`;
                        return (
                          <Button key={model.id} id={rowId}
                            variant={current ? "secondary" : "ghost"}
                            className={cn("h-auto justify-start px-2 py-1.5", styles.row)}
                            tabIndex={tabStop === model.id ? 0 : -1}
                            aria-current={current || undefined}
                            aria-describedby={`${idPrefix}-keyboard`}
                            onFocus={() => setFocusId(model.id)}
                            onKeyDown={(event) => {
                              const index = visibleIds.indexOf(model.id);
                              const next = event.key === "ArrowUp" ? index - 1 :
                                event.key === "ArrowDown" ? index + 1 :
                                event.key === "Home" ? 0 : event.key === "End" ? visibleIds.length - 1 : null;
                              if (next === null) return;
                              event.preventDefault();
                              if (visibleIds[next]) document.getElementById(`${idPrefix}-${visibleIds[next]}`)?.focus();
                            }}
                            onClick={() => {
                              if (current) return;
                              if (!modelId) position.current = { listKey: locationKey, modelId: model.id, search, scrollY: window.scrollY };
                              updateParams({ model: model.id, rule: null });
                            }}>
                            <span className="flex w-full min-w-0 flex-col items-start gap-0.5 whitespace-normal text-left">
                              <span className="flex w-full items-start gap-1"><span className="grow wrap-anywhere">{model.display_name}</span>
                                {current && <Check className="shrink-0" aria-hidden="true" />}
                              </span>
                              {model.source_model_id !== model.display_name && <span className="max-w-full truncate font-mono text-xs text-muted-foreground" title={model.source_model_id}>
                                {model.source_model_id}
                              </span>}
                              <ModelPrices model={model} compact />
                            </span>
                          </Button>
                        );
                      })}
                    </section>
                  ))}
                </CardContent>
                <CardFooter className={cn("shrink-0", styles.jump)}>
                  <Button variant="outline" size="sm" disabled={!selected} onClick={focusDetail} className="w-full">
                    {t("Go to selected model configuration")}<ArrowRight data-icon="inline-end" />
                  </Button>
                </CardFooter>
              </Card>
              <section ref={detail} data-detail-open={modelId ? "" : undefined}
                className={cn("flex min-w-0 flex-col gap-4", styles.detail)}
                aria-label={t("Operation routing")}>
                {modelId && <Button ref={back} variant="outline" className={cn("self-start", selected && styles.back)}
                  onClick={returnToDirectory}>
                  <ArrowLeft data-icon="inline-start" />{t("Back to client models")}
                </Button>}
                {selected ? (
                  <>
                    <Card variant="section" className="shrink-0">
                      <CardHeader>
                        <div className="flex flex-wrap items-start gap-2">
                          <h2 ref={heading} tabIndex={-1} className="min-w-0 wrap-anywhere text-xl font-semibold">{selected.display_name}</h2>
                          <StatusBadge value={selected.enabled} />
                        </div>
                        {selected.source_model_id !== selected.display_name && <CardDescription className="wrap-anywhere font-mono">{selected.source_model_id}</CardDescription>}
                      </CardHeader>
                      <CardContent className="flex flex-col gap-4">
                        <div className="flex flex-wrap items-center gap-2">
                          <NavigationLink to={withReturnTo(`/admin/models/${selected.id}`, origin)}>
                            <Settings2 data-icon="inline-start" />{t("Edit model")}
                          </NavigationLink>
                          <NavigationLink to={withReturnTo(`/admin/models/${selected.id}/pricing`, origin)}>
                            <Calculator data-icon="inline-start" />{t("Configure pricing")}
                          </NavigationLink>
                          <NavigationLink variant="ghost" to={adminPath("/admin/models/new", { copyFrom: selected.id, returnTo: origin })}>
                            <Copy data-icon="inline-start" />{t("Copy model")}
                          </NavigationLink>
                        </div>
                        <ModelPrices model={selected} />
                      </CardContent>
                    </Card>
                    <div className="flex shrink-0 flex-wrap items-center gap-2">
                      {modelRules.length ? <ToggleGroup value={selectedRule ? [selectedRule.id] : []} variant="outline"
                        className="flex-wrap" aria-label={t("Operation routing")}
                        onValueChange={(values) => updateParams({ rule: values[0] ?? null })}>
                        {modelRules.map((rule) => (
                          <ToggleGroupItem key={rule.id} value={rule.id}>
                            {apiOperationLabel(rule.operation)}
                            <StatusBadge value={rule.enabled} />
                          </ToggleGroupItem>
                        ))}
                      </ToggleGroup> : !adding && <p className="text-sm text-muted-foreground">{t("No operations configured yet.")}</p>}
                      <Button variant="outline" disabled={adding || modelRules.length >= API_OPERATIONS.length}
                        onClick={() => updateParams({ rule: "new" })}>
                        <Plus data-icon="inline-start" />{t("Add operation")}
                      </Button>
                      <NavigationLink to={`/admin/models/${selected.id}/onboarding`}>{t("Bulk configure routes")}</NavigationLink>
                    </div>
                    {selectedRule || adding ? (
                      <div className="shrink-0">
                        <OperationRuleDetailPage
                          key={`${selected.id}:${selectedRule?.id ?? "new"}`}
                          embedded
                          modelId={selected.id}
                          ruleId={selectedRule?.id ?? "new"}
                          onCreated={(id) => updateParams({ rule: id }, true)}
                        />
                      </div>
                    ) : modelRules.length > 0 && <EmptyState title={t("Choose an operation")} />}
                  </>
                ) : modelId ? (
                  <h2 ref={heading} tabIndex={-1} className="text-base font-medium">{t("This client model is no longer available.")}</h2>
                ) : <EmptyState title={t("Select a client model")} />}
              </section>
            </div>
          </AsyncResource>
        </TabsContent>
      </Tabs>
    </div>
  );
}
