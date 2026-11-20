import { useState } from "react";
import { toast } from "sonner";
import { ApiError, NetworkError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ChannelCapabilityView, LogicalChannelView } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useBatchUpdateCapabilities, useDiscoverChannelModels, useUpstreamAccesses } from "@/features/admin/api";
import { apiOperationLabel } from "@/lib/permissions";
import { modelDiscoveryInput, supportsModelDiscovery } from "./model-discovery-input";

interface AppendModelDialogProps {
  capabilities: ChannelCapabilityView[];
  channels: LogicalChannelView[];
  refresh: () => Promise<boolean>;
  onClose: () => void;
  onApplied: () => void;
}

export function AppendModelDialog({ capabilities, channels, refresh, onClose, onApplied }: AppendModelDialogProps) {
  const { t } = useI18n();
  const accesses = useUpstreamAccesses();
  const discovery = useDiscoverChannelModels();
  const mutation = useBatchUpdateCapabilities();
  const [input, setInput] = useState("");
  const [sourceId, setSourceId] = useState<string | null>(null);
  const [conflict, setConflict] = useState<"stale" | "missing" | "unknown" | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const model = input.trim();
  const tooLong = [...model].length > 300;
  const rows = capabilities.map((cap) => {
    const channel = channels.find(({ id }) => id === cap.channel_id);
    const access = accesses.data?.find(({ id }) => id === channel?.access_id);
    return { cap, label: `${channel?.name ?? cap.channel_id} / ${apiOperationLabel(cap.settings.operation)}`,
      discoveryInput: channel && access && supportsModelDiscovery(access)
        ? modelDiscoveryInput(channel, access, cap.settings.operation, cap.config_template_id, cap.override_document) : null,
      exists: cap.settings.available_models.includes(model),
    };
  });
  const sources = rows.filter((row) => row.discoveryInput !== null);
  const source = sources.find(({ cap }) => cap.id === sourceId) ?? (sources.length === 1 ? sources[0] : null);
  const additions = model && !tooLong ? rows.filter(({ exists }) => !exists).length : 0;
  const canAppend = additions > 0 && !conflict && !mutation.isPending;

  const apply = async () => {
    if (!canAppend) return;
    try {
      const result = await mutation.mutateAsync({
        items: capabilities.map(({ id, updated_at }) => ({ id, updated_at })),
        changes: { append_model: model },
      });
      toast.success(t("Model added to {count} capabilities.", { count: result.updated_ids.length }));
      onApplied();
      onClose();
    } catch (error) {
      if (error instanceof ApiError && (error.isConflict || error.isNotFound)) setConflict("stale");
      else if (error instanceof NetworkError) setConflict("unknown");
      else toast.error(t(controlPlaneMutationErrorMessage(error, "Could not update capabilities.")));
    }
  };

  const refreshPreview = async () => {
    setRefreshing(true);
    try {
      setConflict(await refresh() ? null : "missing");
      discovery.reset();
    } catch (error) {
      toast.error(t(controlPlaneMutationErrorMessage(error, "Could not load capabilities.")));
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !mutation.isPending && !refreshing) onClose(); }}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t("Append upstream model")}</DialogTitle>
          <DialogDescription>{t("Add one model to the selected catalogs. Existing models and routing rules stay unchanged.")}</DialogDescription>
        </DialogHeader>
        <FieldGroup>
          <Field data-invalid={tooLong}>
            <FieldLabel htmlFor="append-model">{t("Upstream model")}</FieldLabel>
            <Input id="append-model" placeholder={t("Enter an upstream model ID")} value={input}
              aria-invalid={tooLong} disabled={mutation.isPending} onChange={(event) => setInput(event.target.value)} />
            {tooLong && <FieldError>{t("Upstream model must be at most 300 characters.")}</FieldError>}
          </Field>
          {sources.length > 0 && <Field>
            <FieldLabel htmlFor="append-discovery-source">{t("Discovery source")}</FieldLabel>
            <div className="flex flex-wrap items-center gap-2">
              <Select value={source?.cap.id ?? null} disabled={discovery.isPending || mutation.isPending}
                onValueChange={(value) => { setSourceId(value); discovery.reset(); }}>
                <SelectTrigger id="append-discovery-source" className="max-w-full"><SelectValue placeholder={t("Choose a capability")} /></SelectTrigger>
                <SelectContent><SelectGroup>{sources.map(({ cap, label }) => <SelectItem key={cap.id} value={cap.id}>{label}</SelectItem>)}</SelectGroup></SelectContent>
              </Select>
              <Button variant="outline" disabled={!source || discovery.isPending || mutation.isPending}
                onClick={() => { if (source?.discoveryInput) discovery.mutate(source.discoveryInput); }}>{t("Fetch models")}</Button>
            </div>
            {discovery.isError && <FieldError>{t(controlPlaneMutationErrorMessage(discovery.error, "Could not fetch models."))}</FieldError>}
            {discovery.isSuccess && !discovery.data.models.length && <p className="text-sm text-muted-foreground">{t("No models returned.")}</p>}
          </Field>}
          {!!discovery.data?.models.length && <Field>
            <FieldLabel htmlFor="append-discovered">{t("Discovered models")}</FieldLabel>
            <Select value={discovery.data.models.includes(model) ? model : null} onValueChange={setInput} disabled={mutation.isPending}>
              <SelectTrigger id="append-discovered" className="max-w-full"><SelectValue placeholder={t("Choose an upstream model")} /></SelectTrigger>
              <SelectContent><SelectGroup>{discovery.data.models.map((id) => <SelectItem key={id} value={id}>{id}</SelectItem>)}</SelectGroup></SelectContent>
            </Select>
          </Field>}
        </FieldGroup>
        <section className="flex min-w-0 flex-col gap-2" aria-label={t("Preview")}>
          <h3 className="text-sm font-medium">{t("Preview")} · {t("{count} selected", { count: rows.length })}
            {model && !tooLong && ` · ${t("{count} to update", { count: additions })}`}</h3>
          <ul className="flex max-h-52 flex-col gap-2 overflow-y-auto text-sm">
            {rows.map(({ cap, label, exists }) => <li key={cap.id} className="flex min-w-0 flex-wrap justify-between gap-x-2">
              <span className="break-words">{label}</span>
              {model && !tooLong && <span className="text-muted-foreground">{t(exists ? "Already listed" : "Will append")}</span>}
            </li>)}
          </ul>
        </section>
        {conflict && <Alert variant="destructive"><AlertDescription>{t(conflict === "missing"
          ? "A selected capability no longer exists. Close and select the targets again."
          : conflict === "unknown" ? "Connection lost. Refresh the preview to check which catalogs changed."
          : "The selected capabilities changed. Refresh the preview before applying.")}</AlertDescription></Alert>}
        <DialogFooter>
          <Button variant="outline" disabled={mutation.isPending || refreshing} onClick={onClose}>{t("Cancel")}</Button>
          {conflict === "stale" || conflict === "unknown"
            ? <Button disabled={refreshing} onClick={() => void refreshPreview()}>{t("Refresh preview")}</Button>
            : <Button disabled={!canAppend} onClick={() => void apply()}>{t("Apply changes")}</Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
