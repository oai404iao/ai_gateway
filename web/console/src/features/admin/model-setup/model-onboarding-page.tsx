import { useEffect, useRef, useState } from "react";
import { useParams } from "react-router";
import { toast } from "sonner";
import { ApiError, NetworkError } from "@/api/errors";
import type { ControlPlaneModel, OnboardingOperationInput, OperationRuleView, RoutingProfileView } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { AsyncResource, ErrorAlert } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useModels, useOnboardModel, useOperationRules, useReloadOnboarding, useRoutingProfiles } from "@/features/admin/api";
import { OnboardingCandidates } from "./onboarding-candidates";
import { OnboardingPricing, PriceSummary } from "./onboarding-pricing";
import { existingSelection, type PricedSelection } from "./onboarding-price";
import { useConfigurationDraft } from "./use-configuration-draft";

export function ModelOnboardingPage() {
  const { id } = useParams();
  const models = useModels(), profiles = useRoutingProfiles(), rules = useOperationRules();
  const error = models.error ?? profiles.error ?? rules.error;
  if (!models.data || !profiles.data || !rules.data) {
    return <AsyncResource isLoading={models.isLoading || profiles.isLoading || rules.isLoading} error={error}>{null}</AsyncResource>;
  }
  return <>
    {error && <ErrorAlert error={error} />}
    <Workspace key={id ?? "new"} id={id} models={models.data} profiles={profiles.data} rules={rules.data} />
  </>;
}

type Preview = { selection: PricedSelection; rules: OperationRuleView[] };
function snapshot(selection: PricedSelection, profiles: RoutingProfileView[], rules: OperationRuleView[]): Preview {
  const model = selection.model;
  const profile = "existing" in model && profiles.find((p) => p.model_id === model.existing.id);
  return { selection, rules: rules.filter((rule) => profile && rule.model_routing_profile_id === profile.id) };
}
function Workspace({ id, models, profiles, rules }: {
  id?: string; models: ControlPlaneModel[]; profiles: RoutingProfileView[]; rules: OperationRuleView[];
}) {
  const { t } = useI18n();
  const current = models.find((model) => model.id === id);
  const profileFor = (modelId: string) => profiles.find((p) => p.model_id === modelId);
  const [preview, setPreview] = useState<Preview | null>(() => current ? snapshot(existingSelection(current), profiles, rules) : null);
  const [operations, setOperations] = useState<OnboardingOperationInput[]>([]);
  const [conflict, setConflict] = useState<"stale" | "unknown" | null>(null);
  const [revision, setRevision] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const submit = useOnboardModel();
  const reload = useReloadOnboarding();
  const draft = useConfigurationDraft(submit.isPending);
  const heading = useRef<HTMLHeadingElement>(null);
  const pricing = useRef<HTMLDivElement>(null);
  const conflictMessage = useRef<HTMLDivElement>(null);
  const direct = Boolean(id);
  const backTo = current ? `/admin/models?model=${current.id}` : "/admin/models";
  const lockedModel = (model: ControlPlaneModel) => !model.enabled && !profileFor(model.id);
  const unavailable = direct && (!current || lockedModel(current));
  const candidates = operations.flatMap((op) => op.candidates);
  const count = candidates.length;
  const busy = submit.isPending || refreshing;
  useEffect(() => {
    if (preview) heading.current?.focus();
    else pricing.current?.querySelector("input")?.focus();
  }, [preview]);
  useEffect(() => { if (conflict) conflictMessage.current?.focus(); }, [conflict]);

  function review(selection: PricedSelection) {
    if ("existing" in selection.model) {
      const { id } = selection.model.existing;
      const model = models.find((m) => m.id === id);
      if (!model || lockedModel(model)) { setError(new Error(t("Enable this priced model before creating its routing profile."))); return; }
    }
    setError(null); setOperations([]); setPreview(snapshot(selection, profiles, rules)); draft.markDirty();
  }
  async function apply() {
    if (!preview || busy || conflict || !count || count > 100) return;
    setError(null);
    try {
      const { model_id } = await submit.mutateAsync({ model: preview.selection.model, operations });
      draft.markSaved(); toast.success(t("Routing changes applied"));
      draft.navigate(`/admin/models?model=${model_id}`, { replace: true });
    } catch (failure) {
      if (failure instanceof ApiError && failure.isConflict) setConflict("stale");
      else if (failure instanceof NetworkError) setConflict("unknown");
      else setError(failure);
    }
  }
  async function refresh() {
    setRefreshing(true); setError(null);
    try {
      const fresh = await reload();
      const model = direct && fresh.models.find((m) => m.id === id);
      setPreview(model ? snapshot(existingSelection(model), fresh.profiles, fresh.rules) : null);
      setOperations([]); setConflict(null); setRevision((n) => n + 1);
    } catch (failure) { setError(failure); }
    finally { setRefreshing(false); }
  }

  return <div className="flex min-w-0 flex-col gap-5">
    {draft.navigationGuard}
    <PageHeader backTo={backTo} title={direct ? "Bulk configure routes" : "Onboard model"}
      description={direct ? "Append candidates from multiple providers without changing existing pricing or routes." : undefined} />
    {unavailable ? <Alert><AlertTitle>{t(current ? "Enable this priced model before creating its routing profile." : "This client model is no longer available.")}</AlertTitle></Alert> : <>
      {!direct && <div ref={pricing} hidden={!!preview}><OnboardingPricing models={models} catalogRevision={revision} onReview={review} onEdit={draft.markDirty} /></div>}
      {preview && <>
        <Card><CardHeader><CardTitle className="wrap-anywhere font-mono">{preview.selection.name}</CardTitle>
          <CardDescription>{t("existing" in preview.selection.model ? "Use existing pricing without changes" : "catalog" in preview.selection.model ? "Import catalog pricing" : "Create priced model")}{preview.selection.source && ` · ${preview.selection.source}`}</CardDescription></CardHeader>
          <CardContent className="flex flex-col items-start gap-3"><PriceSummary quote={preview.selection.quote} />
            {!direct && <Button variant="outline" size="sm" disabled={busy || !!conflict} onClick={() => { setPreview(null); setOperations([]); setError(null); }}>{t("Edit model and pricing")}</Button>}
          </CardContent></Card>
        <section className="flex min-w-0 flex-col gap-3">
          <h2 ref={heading} tabIndex={-1} className="scroll-mt-20 text-lg font-semibold">{t(direct ? "Routing candidates" : "2 · Routing candidates")}</h2>
          <p className="text-sm text-muted-foreground">{t(direct ? "Review exact matches or choose an advertised upstream alias." : "Exact matches are suggested. Only register a missing model on operations the provider actually supports.")}</p>
          <fieldset disabled={busy || !!conflict} className="min-w-0"><OnboardingCandidates key={revision} modelName={preview.selection.name} rules={preview.rules}
            allowRegistration={!direct} onChange={setOperations} onEdit={draft.markDirty} /></fieldset>
        </section>
        {conflict && <div tabIndex={-1} ref={conflictMessage}><Alert variant="destructive"><AlertTitle>{t(conflict === "stale" ? "Configuration changed. Nothing was applied." : "Result unknown. Reload the preview before trying again.")}</AlertTitle>
          <AlertDescription>{t("Reload the current prices and routes, then review your selection again.")}
            <Button variant="outline" disabled={busy} onClick={() => void refresh()}>{t("Reload preview")}</Button>
          </AlertDescription></Alert></div>}
        <footer className="sticky bottom-[var(--console-dock-clearance,0px)] z-10 flex min-w-0 flex-wrap items-center justify-between gap-3 border-y bg-background py-3">
          <p role="status" className="text-sm">{t("{ops} operations · {routes} candidates · {models} catalog additions", {
            ops: operations.length, routes: count, models: candidates.filter((c) => c.register_model).length,
          })}{count > 100 && <span className="block text-destructive">{t("Select at most 100 candidates.")}</span>}</p>
          <Button disabled={busy || !!conflict || !count || count > 100} onClick={() => void apply()}>{t(direct ? "Apply route additions" : "Apply onboarding")}</Button>
        </footer>
      </>}
    </>}
    {error != null && <ErrorAlert error={error} />}
  </div>;
}
