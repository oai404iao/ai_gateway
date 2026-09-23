import { useSearchParams } from "react-router";
import { ArrowLeft, Calculator, Copy, Plus, Settings2 } from "lucide-react";
import { useI18n } from "@/app/i18n";
import { AsyncResource, ErrorAlert } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { StatusBadge } from "@/components/shared/status-badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { useModels, useOperationRules, useRoutingProfiles } from "@/features/admin/api";
import { CatalogPage } from "@/features/admin/catalog/catalog-page";
import { OperationRuleDetailPage } from "@/features/admin/routing/operation-rules/operation-rule-detail-page";
import { API_OPERATIONS, apiOperationLabel } from "@/lib/permissions";
import { groupModelsByProvider } from "./model-groups";
import { adminPath } from "@/features/admin/model-setup/model-setup-navigation";
import { formatCompactTokens, formatDecimal } from "@/lib/formatters";
import type { ControlPlaneModel } from "@/api/types";
import { NavigationLink } from "@/components/shared/navigation-link";
import { usePageOrigin, withReturnTo } from "@/lib/page-navigation";

function ModelPrices({ model }: { model: ControlPlaneModel }) {
  const { t } = useI18n();
  return (
    <span className="flex w-full flex-col gap-1 text-xs">
      <span>{t("Base prices · {currency} / {tokens} tokens", {
        currency: "USD", tokens: formatCompactTokens(model.price_unit_tokens),
      })}</span>
      <span className="grid grid-cols-3 gap-3">
        {([
          ["Input price", model.input_unit_price],
          ["Cache hit price", model.cached_input_unit_price],
          ["Output price", model.output_unit_price],
        ] as const).map(([label, value]) => (
          <span key={label} className="flex flex-col gap-1">
            <span>{t(label)}</span>
            <span className="tabular-nums">{formatDecimal(value, 12)}</span>
          </span>
        ))}
      </span>
    </span>
  );
}

export function ModelsPage() {
  const { t } = useI18n();
  const origin = usePageOrigin();
  const [params, setParams] = useSearchParams();
  const models = useModels();
  const profiles = useRoutingProfiles();
  const rules = useOperationRules();
  const hasLists = Boolean(models.data && profiles.data && rules.data);
  const listError = models.error ?? profiles.error ?? rules.error;
  const groups = groupModelsByProvider(models.data ?? [], t("Unspecified provider"));
  const selected = models.data?.find((model) => model.id === params.get("model"));
  const profile = profiles.data?.find((item) => item.model_id === selected?.id);
  const modelRules = (rules.data ?? []).filter((rule) =>
    profile && rule.model_routing_profile_id === profile.id,
  );
  const ruleId = params.get("rule");
  const selectedRule = modelRules.find((rule) => rule.id === ruleId);
  const updateParams = (values: Record<string, string | null>) => {
    setParams((current) => {
      const next = new URLSearchParams(current);
      for (const [key, value] of Object.entries(values)) {
        if (value === null) next.delete(key);
        else next.set(key, value);
      }
      return next;
    });
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Model configuration")}
        description={t("Select a client model, add an operation, then configure its routing rules.")}
        actions={
          <NavigationLink to={withReturnTo("/admin/models/new", origin)} variant="default" size="default">
            <Plus data-icon="inline-start" />{t("New client model")}
          </NavigationLink>
        }
      />
      <Tabs value={params.get("view") === "prices" ? "prices" : "models"}
        onValueChange={(value) => updateParams({ view: String(value) })}>
        <TabsList>
          <TabsTrigger value="models">{t("Client models")}</TabsTrigger>
          <TabsTrigger value="prices">{t("Price sync")}</TabsTrigger>
        </TabsList>
        <TabsContent value="prices"><CatalogPage embedded /></TabsContent>
        <TabsContent value="models">
          {hasLists && listError && <ErrorAlert error={listError} />}
          <AsyncResource
            isLoading={models.isLoading || profiles.isLoading || rules.isLoading}
            error={hasLists ? null : listError}
          >
            <div className="grid min-w-0 items-start gap-5 xl:grid-cols-[20rem_minmax(0,1fr)]">
              <Card className={cn("min-w-0", selected && "hidden xl:flex")}>
                <CardHeader>
                  <CardTitle>{t("Client models")}</CardTitle>
                  <CardDescription>{t("Choose a model to configure its operations.")}</CardDescription>
                </CardHeader>
                <CardContent className="flex flex-col gap-4">
                  {groups.length === 0 && <EmptyState title={t("No records")} />}
                  {groups.map((group) => (
                    <div key={group.provider} className="flex flex-col gap-2">
                      <p className="text-sm text-muted-foreground">{group.provider}</p>
                      {group.models.map((model) => (
                        <Button key={model.id}
                          variant={selected?.id === model.id ? "secondary" : "ghost"}
                          className="h-auto justify-start"
                          aria-pressed={selected?.id === model.id}
                          onClick={() => updateParams({ model: model.id, rule: null })}>
                          <span className="flex min-w-0 w-full flex-col items-start gap-1 whitespace-normal text-left">
                            <span className="max-w-full truncate">{model.display_name}</span>
                            <span className="max-w-full truncate text-xs">{model.source_model_id}</span>
                            <ModelPrices model={model} />
                          </span>
                        </Button>
                      ))}
                    </div>
                  ))}
                </CardContent>
              </Card>
              <section className={cn("min-w-0 flex-col gap-4", selected ? "flex" : "hidden xl:flex")} aria-label={t("Operation routing")}>
                {selected ? (
                  <>
                    <Button variant="outline" className="self-start xl:hidden"
                      onClick={() => updateParams({ model: null, rule: null })}>
                      <ArrowLeft data-icon="inline-start" />{t("Back to client models")}
                    </Button>
                    <Card>
                      <CardHeader>
                        <CardTitle>{selected.display_name}</CardTitle>
                        <CardDescription>{selected.source_model_id}</CardDescription>
                      </CardHeader>
                      <CardContent className="flex flex-wrap items-center gap-2">
                        <ModelPrices model={selected} />
                        <StatusBadge value={selected.enabled} />
                        <NavigationLink to={withReturnTo(`/admin/models/${selected.id}`, origin)}>
                          <Settings2 data-icon="inline-start" />{t("Edit model")}
                        </NavigationLink>
                        <NavigationLink to={withReturnTo(`/admin/models/${selected.id}/pricing`, origin)}>
                          <Calculator data-icon="inline-start" />{t("Configure pricing")}
                        </NavigationLink>
                        <NavigationLink to={adminPath("/admin/models/new", { copyFrom: selected.id, returnTo: origin })}>
                          <Copy data-icon="inline-start" />{t("Copy model")}
                        </NavigationLink>
                      </CardContent>
                    </Card>
                    <div className="flex flex-wrap gap-2">
                      <ToggleGroup value={selectedRule ? [selectedRule.id] : []} variant="outline"
                        className="flex-wrap" aria-label={t("Operation routing")}
                        onValueChange={(values) => updateParams({ rule: values[0] ?? null })}>
                      {modelRules.map((rule) => (
                        <ToggleGroupItem key={rule.id} value={rule.id}>
                          {apiOperationLabel(rule.operation)}
                          <StatusBadge value={rule.enabled} />
                        </ToggleGroupItem>
                      ))}
                      </ToggleGroup>
                      <Button variant="outline" disabled={modelRules.length >= API_OPERATIONS.length}
                        onClick={() => updateParams({ rule: "new" })}>
                        <Plus data-icon="inline-start" />{t("Add operation")}
                      </Button>
                    </div>
                    {selectedRule || (ruleId === "new" && modelRules.length < API_OPERATIONS.length) ? (
                      <OperationRuleDetailPage
                        key={`${selected.id}:${selectedRule?.id ?? "new"}`}
                        embedded
                        modelId={selected.id}
                        ruleId={selectedRule?.id ?? "new"}
                        onCreated={(id) => updateParams({ rule: id })}
                      />
                    ) : <EmptyState title={t("Choose or add an operation")} />}
                  </>
                ) : <EmptyState title={t("Select a client model")} />}
              </section>
            </div>
          </AsyncResource>
        </TabsContent>
      </Tabs>
    </div>
  );
}
