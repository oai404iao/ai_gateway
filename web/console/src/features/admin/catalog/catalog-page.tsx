import { useState } from "react";
import { toast } from "sonner";
import { Download } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import {
  useApplyCatalogModels,
  useModelSyncPreview,
} from "@/features/admin/api";
import type { ModelSyncPreviewModel, ModelSyncSelection } from "@/api/types";
import { ApiError } from "@/api/errors";
import { formatDateTime } from "@/lib/dates";
import { formatTokens } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";

function actionVariant(action: ModelSyncPreviewModel["action"]) {
  if (action === "price_update") return "secondary";
  if (action === "import") return "default";
  return "outline";
}

function BillingDetails({ model }: { model: ModelSyncPreviewModel }) {
  const { t } = useI18n();
  const { long_context_tiers, request_multipliers } = model.advanced_billing;
  const prices = [
    { label: t("Base prices"), rates: model },
    ...long_context_tiers.map((tier) => ({
      label: t("Input tokens ≥ {threshold}", { threshold: formatTokens(tier.input_tokens_threshold) }),
      rates: tier,
    })),
  ];
  return (
    <Collapsible data-row-click-ignore>
      <CollapsibleTrigger render={<Button variant="outline" size="xs" />}>
        {t("Billing details")}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="flex flex-col gap-2 pt-2">
          <p>{t("USD per million tokens")}</p>
          {prices.map(({ label, rates }) => (
            <div key={label}>
              <p className="font-medium">{label}</p>
              <dl>
                {([
                  ["Input unit price", rates.input_unit_price],
                  ["Cached input unit price", rates.cached_input_unit_price],
                  ["Cache write unit price", rates.cache_write_unit_price],
                  ["Output unit price", rates.output_unit_price ?? model.output_unit_price],
                ] as const).map(([name, value]) => (
                  <div key={name} className="flex justify-between gap-3">
                    <dt className="text-muted-foreground">{t(name)}</dt>
                    <dd className="tabular-nums">{value}</dd>
                  </div>
                ))}
              </dl>
            </div>
          ))}
          {request_multipliers.length > 0 && (
            <div>
              <p className="font-medium">{t("Request multipliers")}</p>
              <ul>
                {request_multipliers.map(({ json_pointer, value, multiplier }) => (
                  <li key={JSON.stringify([json_pointer, value])} className="whitespace-normal break-all">
                    <code>{json_pointer} = {JSON.stringify(value)}</code> × {multiplier}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

export function CatalogPage({ embedded = false }: { embedded?: boolean } = {}) {
  const preview = useModelSyncPreview();
  const applyCatalogModels = useApplyCatalogModels();
  const [providerIds, setProviderIds] = useState("");
  const [selected, setSelected] = useState<Set<ModelSyncPreviewModel>>(new Set());
  const previewData = preview.data?.models;
  const { t } = useI18n();

  const fetchPreview = async () => {
    try {
      await preview.mutateAsync({
        provider_ids: providerIds
          .split(/[,\s]+/)
          .map((item) => item.trim())
          .filter(Boolean),
      });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Preview failed"));
    } finally {
      setSelected(new Set());
    }
  };

  const runApply = async () => {
    if (selected.size === 0) return;
    const selections: ModelSyncSelection[] = [...selected].map(
      ({ provider_id, model_id, preview_token }) => ({ provider_id, model_id, preview_token }),
    );
    try {
      const result = await applyCatalogModels.mutateAsync({ selections });
      toast.success(
        t("Imported {imported}, updated {updated} model billing configuration(s).", {
          imported: result.imported_count,
          updated: result.updated_count,
        }),
      );
      await fetchPreview();
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        preview.reset();
        setSelected(new Set());
        toast.error(t("Nothing was applied. Fetch a fresh preview and review your selections again."));
        return;
      }
      toast.error(error instanceof Error ? error.message : t("Apply failed"));
    }
  };

  const toggle = (model: ModelSyncPreviewModel) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(model)) next.delete(model);
      else next.add(model);
      return next;
    });
  };

  const columns: Column<ModelSyncPreviewModel>[] = [
    {
      key: "selected",
      header: t("Select"),
      render: (model) => (
        <Checkbox
          aria-label={`${t("Select")} ${model.model_id}`}
          checked={selected.has(model)}
          onCheckedChange={() => toggle(model)}
        />
      ),
      className: "w-12",
    },
    {
      key: "model",
      header: t("Model"),
      render: (model) => (
        <span className="flex flex-col">
          <span className="font-medium">{model.display_name}</span>
          <span className="text-xs text-muted-foreground font-mono">{model.model_id}</span>
        </span>
      ),
    },
    { key: "provider", header: t("Provider"), render: (model) => model.provider_name },
    {
      key: "input",
      header: t("Input price"),
      render: (model) => (
        <div className="flex flex-col items-start gap-1">
          <span className="tabular-nums">{model.input_unit_price}</span>
          <BillingDetails model={model} />
        </div>
      ),
    },
    {
      key: "tiers",
      header: t("Long-context tiers"),
      render: (model) => model.advanced_billing.long_context_tiers.length,
    },
    {
      key: "multipliers",
      header: t("Request multipliers"),
      render: (model) => model.advanced_billing.request_multipliers.length,
    },
    {
      key: "action",
      header: t("Action"),
      render: (model) => (
        <Badge variant={actionVariant(model.action)}>
          {t(model.action.replace("_", " "))}
        </Badge>
      ),
    },
  ];

  const importable = previewData?.filter((model) => model.action === "import") ?? [];
  const updatable = previewData?.filter((model) => model.action === "price_update") ?? [];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        embedded={embedded}
        title={t("Price sync")}
        description={t(
          "Preview, import, or explicitly update models.dev prices and catalog billing rules.",
        )}
      />
      <Card>
        <CardHeader>
          <CardTitle>{t("Preview")}</CardTitle>
          <CardDescription>
            {t("Fetch the models.dev catalog, optionally filtered by provider ids.")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex flex-col gap-4">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="providers">{t("Provider ids (optional)")}</FieldLabel>
                <Input
                  id="providers"
                  value={providerIds}
                  onChange={(event) => setProviderIds(event.target.value)}
                  placeholder="openai, anthropic"
                />
              </Field>
            </FieldGroup>
            <div className="flex flex-wrap gap-2">
              <Button onClick={fetchPreview} disabled={preview.isPending || applyCatalogModels.isPending}>
                {preview.isPending ? <Spinner data-icon="inline-start" /> : <Download data-icon="inline-start" />}
                {t("Fetch preview")}
              </Button>
              <Button
                variant="secondary"
                onClick={runApply}
                disabled={preview.isPending || applyCatalogModels.isPending || selected.size === 0}
              >
                {applyCatalogModels.isPending ? <Spinner data-icon="inline-start" /> : null}
                {t("Apply selected ({count})", { count: selected.size })}
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      {previewData ? (
        <Card>
          <CardHeader>
            <CardTitle>{t("Preview results")}</CardTitle>
            <CardDescription>
              {t("{count} catalog models.", { count: previewData.length })}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <AsyncResource
              isLoading={false}
              error={null}
              isEmpty={previewData.length === 0}
              emptyTitle={t("No catalog models")}
              emptyDescription={t("No models matched the preview request.")}
            >
              <ResourceTable
                columns={columns}
                rows={previewData}
                rowKey={(model) => JSON.stringify([model.provider_id, model.model_id])}
                onRowClick={toggle}
              />
              <p className="mt-2 text-xs text-muted-foreground">
                {t(
                  "Select rows to import new models or update existing model prices and catalog billing rules.",
                )}{" "}
                {t("{importable} new, {updatable} updatable, {selected} selected.", {
                  importable: importable.length,
                  updatable: updatable.length,
                  selected: selected.size,
                })}
              </p>
            </AsyncResource>
          </CardContent>
        </Card>
      ) : null}

      {preview.data ? (
        <p className="text-xs text-muted-foreground">
          {t(
            "Preview fetched {time}. Excluded: {missing} missing prices, {invalid} invalid, {oversized} oversized metadata.",
            {
              time: formatDateTime(preview.data.fetched_at),
              missing: preview.data.excluded_missing_prices,
              invalid: preview.data.excluded_invalid_models,
              oversized: preview.data.excluded_oversized_metadata,
            },
          )}
        </p>
      ) : null}
    </div>
  );
}
