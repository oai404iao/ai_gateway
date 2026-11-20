import { useState } from "react";
import type { ControlPlaneModel, ModelInput, ModelSyncPreviewModel } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { DecimalField } from "@/components/shared/decimal-field";
import { ErrorAlert } from "@/components/shared/async-resource";
import { JsonViewer } from "@/components/shared/json-viewer";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useModelSyncPreview } from "@/features/admin/api";
import { isNonNegativeRustDecimal } from "@/lib/decimal";
import { formatTokens } from "@/lib/formatters";

import { existingSelection, priceFields as rates, type PricedSelection, type Quote, type Rates } from "./onboarding-price";

export function PriceSummary({ quote }: { quote: Quote }) {
  const { t } = useI18n();
  const advanced = quote.advanced_billing;
  return <div className="flex min-w-0 flex-col gap-2">
    <p className="text-sm font-medium">{t("Base prices · {currency} / {tokens} tokens", { currency: "USD", tokens: formatTokens(quote.price_unit_tokens) })}</p>
    <dl className="grid grid-cols-2 gap-3 sm:grid-cols-4">{rates.map(([key, label]) => <div className="min-w-0" key={key}>
      <dt className="text-xs text-muted-foreground">{t(label)}</dt><dd className="wrap-anywhere text-sm tabular-nums">{quote[key]}</dd>
    </div>)}</dl>
    {advanced && (advanced.long_context_tiers.length || advanced.request_multipliers.length || advanced.time_multipliers?.length) ?
      <div className="text-sm">{t("Advanced billing")}<JsonViewer value={advanced} /></div> : null}
  </div>;
}

export function OnboardingPricing({ models, onReview, onEdit, catalogRevision }: {
  models: ControlPlaneModel[]; onReview: (selection: PricedSelection) => void; onEdit: () => void; catalogRevision: number;
}) {
  const { t } = useI18n();
  const [name, setName] = useState("");
  const [method, setMethod] = useState("manual");
  const [copy, setCopy] = useState<ControlPlaneModel | null>(null);
  const [catalog, setCatalog] = useState<{ row: ModelSyncPreviewModel; revision: number } | null>(null);
  const [values, setValues] = useState<Rates>({ input_unit_price: "", output_unit_price: "", cached_input_unit_price: "0", cache_write_unit_price: "0" });
  const [unit, setUnit] = useState("1000000");
  const [submitted, setSubmitted] = useState(false);
  const id = name.trim();
  const local = models.find((model) => model.source_model_id === id);
  const catalogRow = catalog?.row.model_id === id && catalog.revision === catalogRevision ? catalog.row : null;
  const quote = local ?? (method === "copy" ? copy : method === "catalog" ? catalogRow && { ...catalogRow, price_unit_tokens: 1000000 } : null);
  const complete = id && (quote || (method === "manual" && rates.every(([key]) => isNonNegativeRustDecimal(values[key])) && Number.isSafeInteger(Number(unit)) && Number(unit) > 0));
  function review() {
    setSubmitted(true);
    if (!complete) return;
    if (local) return onReview(existingSelection(local));
    if (method === "catalog" && catalogRow && quote) {
      const { model_id, provider_id, preview_token } = catalogRow;
      return onReview({ name: id, source: `${catalogRow.provider_name} · ${provider_id}`, quote, model: { catalog: { model_id, provider_id, preview_token } } });
    }
    const template = method === "copy" ? copy : null;
    const input: ModelInput = {
      source_model_id: id, display_name: id, enabled: true, provider_name: template?.provider_name ?? null,
      price_unit_tokens: template?.price_unit_tokens ?? Number(unit), price_effective_at: new Date().toISOString(),
      advanced_billing: template?.advanced_billing,
      input_unit_price: template?.input_unit_price ?? values.input_unit_price,
      output_unit_price: template?.output_unit_price ?? values.output_unit_price,
      cache_write_unit_price: template?.cache_write_unit_price ?? values.cache_write_unit_price,
      cached_input_unit_price: template?.cached_input_unit_price ?? values.cached_input_unit_price,
    };
    onReview({ name: id, source: template ? t("Copied from {model}", { model: template.source_model_id }) : t("Enter prices"), quote: input, model: { create: input } });
  }
  return <Card><CardHeader><CardTitle>{t("1 · Model and pricing")}</CardTitle></CardHeader><CardContent>
    <form className="flex flex-col gap-4" onSubmit={(event) => { event.preventDefault(); review(); }}>
      <Field><FieldLabel htmlFor="onboard-name">{t("Client model id")}</FieldLabel>
        <Input id="onboard-name" required value={name} onChange={(e) => { setName(e.target.value); setCatalog(null); setSubmitted(false); onEdit(); }} />
      </Field>
      {local ? <p role="status" className="text-sm">{t("Use existing pricing without changes")}</p> : <>
        <Field><FieldLabel htmlFor="price-source">{t("Price source")}</FieldLabel>
          <Select value={method} onValueChange={(value) => { setMethod(value); setCatalog(null); setSubmitted(false); onEdit(); }}>
            <SelectTrigger id="price-source"><SelectValue /></SelectTrigger><SelectContent><SelectGroup>
              <SelectItem value="manual">{t("Enter prices")}</SelectItem><SelectItem value="copy">{t("Copy local pricing")}</SelectItem>
              <SelectItem value="catalog">{t("Import catalog pricing")}</SelectItem>
            </SelectGroup></SelectContent></Select>
        </Field>
        {method === "manual" && <FieldGroup className="grid gap-4 sm:grid-cols-2">
          <Field className="sm:col-span-2"><FieldLabel htmlFor="onboard-unit">{t("Price unit tokens")}</FieldLabel>
            <Input id="onboard-unit" type="number" min="1" step="1" required value={unit} onChange={(e) => { setUnit(e.target.value); onEdit(); }} /></Field>
          {rates.map(([key, label]) => <DecimalField key={key} label={`${t(label)} (USD)`} value={values[key]} required
            error={submitted && !isNonNegativeRustDecimal(values[key]) ? t("Enter a non-negative decimal.") : undefined}
            onChange={(value) => { setValues({ ...values, [key]: value }); onEdit(); }} />)}
        </FieldGroup>}
        {method === "copy" && <Field><FieldLabel htmlFor="onboard-copy">{t("Copy pricing from model")}</FieldLabel>
          <Select value={copy?.id ?? ""} onValueChange={(value) => { setCopy(models.find((m) => m.id === value) ?? null); onEdit(); }}>
            <SelectTrigger id="onboard-copy" className="max-w-full"><SelectValue placeholder={t("Choose a model")} /></SelectTrigger>
            <SelectContent><SelectGroup>{models.map((m) => <SelectItem key={m.id} value={m.id}>{m.source_model_id}</SelectItem>)}</SelectGroup></SelectContent>
          </Select></Field>}
        {method === "catalog" && <CatalogChoice key={`${id}:${catalogRevision}`} name={id} selected={catalogRow}
          onSelect={(row) => { setCatalog(row && { row, revision: catalogRevision }); onEdit(); }} />}
      </>}
      {quote && <PriceSummary quote={quote} />}
      {submitted && !complete && <p role="alert" className="text-sm text-destructive">{t("Enter a model ID and confirm its pricing first.")}</p>}
      <Button type="submit" className="self-start">{t("Review routing candidates")}</Button>
    </form>
  </CardContent></Card>;
}

function CatalogChoice({ name, selected, onSelect }: { name: string; selected: ModelSyncPreviewModel | null; onSelect: (row: ModelSyncPreviewModel | null) => void }) {
  const { t } = useI18n();
  const preview = useModelSyncPreview();
  const rows = preview.data?.models.filter((row) => row.model_id === name && row.action === "import") ?? [];
  return <div className="flex flex-col items-start gap-3">
    <Button type="button" variant="outline" disabled={!name || preview.isPending} onClick={() => {
      onSelect(null); preview.mutate({ provider_ids: [] });
    }}>{t("Find exact catalog model")}</Button>
    {preview.error && <ErrorAlert error={preview.error} />}
    {preview.isSuccess && !rows.length && <p role="status" className="text-sm">{t("No new catalog entry matches this model ID.")}</p>}
    {!!rows.length && <Field><FieldLabel htmlFor="onboard-catalog">{t("Catalog entry")}</FieldLabel>
      <Select value={selected?.provider_id ?? ""} onValueChange={(provider) => onSelect(rows.find((row) => row.provider_id === provider) ?? null)}>
        <SelectTrigger id="onboard-catalog"><SelectValue placeholder={t("Choose a provider")} /></SelectTrigger><SelectContent><SelectGroup>
          {rows.map((row) => <SelectItem key={row.provider_id} value={row.provider_id}>{row.provider_name} · {row.provider_id}</SelectItem>)}
        </SelectGroup></SelectContent></Select></Field>}
  </div>;
}
