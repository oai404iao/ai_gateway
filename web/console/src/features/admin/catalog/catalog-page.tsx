import { useState } from "react";
import { toast } from "sonner";
import { Download, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import {
  useImportModels,
  useModelSyncPreview,
  useSyncModelPrices,
} from "@/features/admin/api";
import type { ModelSyncPreviewModel, ModelSyncSelection } from "@/api/types";
import { formatDateTime } from "@/lib/dates";
import { formatDecimal } from "@/lib/formatters";

function actionVariant(action: ModelSyncPreviewModel["action"]) {
  if (action === "price_update") return "secondary";
  if (action === "import") return "default";
  return "outline";
}

export function CatalogPage() {
  const preview = useModelSyncPreview();
  const syncPrices = useSyncModelPrices();
  const importModels = useImportModels();
  const [providerIds, setProviderIds] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [previewData, setPreviewData] = useState<ModelSyncPreviewModel[] | null>(null);

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
      toast.error(error instanceof Error ? error.message : "Preview failed");
    }
  };

  const runSync = async () => {
    try {
      const result = await syncPrices.mutateAsync();
      toast.success(
        `Synced ${result.updated_count} price(s)` +
          (result.unavailable_count ? `, ${result.unavailable_count} unavailable` : ""),
      );
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Sync failed");
    }
  };

  const runImport = async () => {
    if (selected.size === 0) return;
    const selections: ModelSyncSelection[] = [...selected].map((key) => {
      const [provider_id, model_id] = key.split("|");
      return { provider_id, model_id };
    });
    try {
      const result = await importModels.mutateAsync({ selections });
      toast.success(`Imported ${result.model_count} model(s)`);
      setSelected(new Set());
      await fetchPreview();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Import failed");
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
      key: "model",
      header: "Model",
      render: (model) => (
        <span className="flex flex-col">
          <span className="font-medium">{model.display_name}</span>
          <span className="text-xs text-muted-foreground font-mono">{model.model_id}</span>
        </span>
      ),
    },
    { key: "provider", header: "Provider", render: (model) => model.provider_name },
    {
      key: "input",
      header: "Input price",
      render: (model) => formatDecimal(model.input_unit_price),
    },
    {
      key: "action",
      header: "Action",
      render: (model) => <Badge variant={actionVariant(model.action)}>{model.action.replace("_", " ")}</Badge>,
    },
  ];

  const importable = previewData?.filter((model) => model.action === "import") ?? [];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Catalog"
        description="Preview and import models.dev prices. Existing imported models can be refreshed."
      />
      <Card>
        <CardHeader>
          <CardTitle>Preview</CardTitle>
          <CardDescription>
            Fetch the models.dev catalog, optionally filtered by provider ids.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex flex-col gap-4">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="providers">Provider ids (optional)</FieldLabel>
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
                Fetch preview
              </Button>
              <Button variant="outline" onClick={runSync} disabled={syncPrices.isPending}>
                {syncPrices.isPending ? <Spinner data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}
                Sync prices
              </Button>
              <Button
                variant="secondary"
                onClick={runImport}
                disabled={importModels.isPending || selected.size === 0}
              >
                {importModels.isPending ? <Spinner data-icon="inline-start" /> : null}
                Import selected ({selected.size})
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      {previewData ? (
        <Card>
          <CardHeader>
            <CardTitle>Preview results</CardTitle>
            <CardDescription>{previewData.length} catalog models.</CardDescription>
          </CardHeader>
          <CardContent>
            <AsyncResource
              isLoading={false}
              error={null}
              isEmpty={previewData.length === 0}
              emptyTitle="No catalog models"
              emptyDescription="No models matched the preview request."
            >
              <ResourceTable
                columns={columns}
                rows={previewData}
                rowKey={(model) => `${model.provider_id}|${model.model_id}`}
                onRowClick={(model) => model.action === "import" && toggle(model)}
              />
              <p className="mt-2 text-xs text-muted-foreground">
                Click import-eligible rows to select them, then import.
                {importable.length} importable, {selected.size} selected.
              </p>
            </AsyncResource>
          </CardContent>
        </Card>
      ) : null}

      {preview.data ? (
        <p className="text-xs text-muted-foreground">
          Preview fetched {formatDateTime(preview.data.fetched_at)}. Excluded:{" "}
          {preview.data.excluded_missing_prices} missing prices,{" "}
          {preview.data.excluded_invalid_models} invalid,{" "}
          {preview.data.excluded_oversized_metadata} oversized metadata,{" "}
          {preview.data.unavailable_existing_count} unavailable existing.
        </p>
      ) : null}
    </div>
  );
}
