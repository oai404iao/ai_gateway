import { useState } from "react";
import { toast } from "sonner";
import { Download } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
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
import { formatDateTime } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";
import { useI18n } from "@/app/i18n";

function actionVariant(action: ModelSyncPreviewModel["action"]) {
  if (action === "price_update") return "secondary";
  if (action === "import") return "default";
  return "outline";
}

export function CatalogPage() {
  const preview = useModelSyncPreview();
  const applyCatalogModels = useApplyCatalogModels();
  const [providerIds, setProviderIds] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [previewData, setPreviewData] = useState<ModelSyncPreviewModel[] | null>(null);
  const { t } = useI18n();

  const fetchPreview = async () => {
    try {
      const result = await preview.mutateAsync({
        provider_ids: providerIds
          .split(/[,\s]+/)
          .map((item) => item.trim())
          .filter(Boolean),
      });
      setPreviewData(result.models);
      setSelected(new Set());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Preview failed"));
    }
  };

  const runApply = async () => {
    if (selected.size === 0) return;
    const selections: ModelSyncSelection[] = [...selected].map((key) => {
      const [provider_id, model_id] = key.split("|");
      return { provider_id, model_id };
    });
    try {
      const result = await applyCatalogModels.mutateAsync({ selections });
      toast.success(
        t("Imported {imported}, updated {updated} model billing configuration(s).", {
          imported: result.imported_count,
          updated: result.updated_count,
        }),
      );
      setSelected(new Set());
      await fetchPreview();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Apply failed"));
    }
  };

  const toggle = (model: ModelSyncPreviewModel) => {
    const key = `${model.provider_id}|${model.model_id}`;
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const columns: Column<ModelSyncPreviewModel>[] = [
    {
      key: "selected",
      header: t("Select"),
      render: (model) => {
        const key = `${model.provider_id}|${model.model_id}`;
        return (
          <Checkbox
            aria-label={`${t("Select")} ${model.model_id}`}
            checked={selected.has(key)}
            onClick={(event) => event.stopPropagation()}
            onCheckedChange={() => toggle(model)}
          />
        );
      },
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
      render: (model) => formatDecimal(model.input_unit_price),
    },
    {
      key: "tiers",
      header: t("Long-context tiers"),
      render: (model) => model.advanced_billing.long_context_tiers.length,
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
        title={t("Catalog")}
        description={t(
          "Preview, import, or explicitly update models.dev prices and long-context tiers.",
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
              <Button onClick={fetchPreview} disabled={preview.isPending}>
                {preview.isPending ? <Spinner data-icon="inline-start" /> : <Download data-icon="inline-start" />}
                {t("Fetch preview")}
              </Button>
              <Button
                variant="secondary"
                onClick={runApply}
                disabled={applyCatalogModels.isPending || selected.size === 0}
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
                rowKey={(model) => `${model.provider_id}|${model.model_id}`}
                onRowClick={toggle}
              />
              <p className="mt-2 text-xs text-muted-foreground">
                {t(
                  "Select rows to import new models or update existing model prices and long-context tiers.",
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
