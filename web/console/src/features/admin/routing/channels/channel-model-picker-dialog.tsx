import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { EmptyState } from "@/components/shared/empty-state";
import { useI18n } from "@/app/i18n";

interface ChannelModelPickerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  models: string[];
  currentModels: string[];
  onApply: (models: string[]) => void;
}

export function ChannelModelPickerDialog({
  open,
  onOpenChange,
  models,
  currentModels,
  onApply,
}: ChannelModelPickerDialogProps) {
  const { t } = useI18n();
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const rows = useMemo(
    () => models.map((model, index) => ({ id: `channel_model_${index}`, model })),
    [models],
  );

  useEffect(() => {
    if (!open) {
      setSearch("");
      return;
    }
    const discovered = new Set(models);
    setSelected(new Set(currentModels.filter((model) => discovered.has(model))));
  }, [currentModels, models, open]);

  const normalizedSearch = search.trim().toLocaleLowerCase();
  const filteredRows = useMemo(
    () =>
      rows.filter(
        ({ model }) =>
          !normalizedSearch || model.toLocaleLowerCase().includes(normalizedSearch),
      ),
    [normalizedSearch, rows],
  );

  const toggle = (model: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(model)) next.delete(model);
      else next.add(model);
      return next;
    });
  };

  const selectAllResults = () => {
    setSelected((current) => {
      const next = new Set(current);
      for (const { model } of filteredRows) next.add(model);
      return next;
    });
  };

  const apply = () => {
    const discovered = new Set(models);
    const next = currentModels.filter(
      (model) => !discovered.has(model) || selected.has(model),
    );
    const included = new Set(next);
    for (const model of models) {
      if (selected.has(model) && !included.has(model)) {
        included.add(model);
        next.push(model);
      }
    }
    onApply(next);
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("Select upstream models")}</DialogTitle>
          <DialogDescription>
            {t(
              "Choose models returned by the upstream GET /v1/models endpoint. Applying the selection updates discovered entries while preserving manually entered models that were not returned.",
            )}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-4">
          <Field>
            <FieldLabel htmlFor="channel-model-search">{t("Search models")}</FieldLabel>
            <Input
              id="channel-model-search"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("Search by model ID")}
            />
          </Field>

          <div className="flex flex-wrap items-center justify-between gap-2">
            <FieldDescription>
              {t("{count} models fetched; {selected} selected.", {
                count: models.length,
                selected: selected.size,
              })}
            </FieldDescription>
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={selectAllResults}
                disabled={filteredRows.length === 0}
              >
                {t("Select all results")}
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setSelected(new Set())}
                disabled={selected.size === 0}
              >
                {t("Clear selection")}
              </Button>
            </div>
          </div>

          <ScrollArea className="h-[min(50svh,28rem)] rounded-md border">
            {filteredRows.length > 0 ? (
              <FieldSet className="p-3">
                <FieldLegend variant="label">{t("Discovered models")}</FieldLegend>
                <FieldGroup data-slot="checkbox-group" className="gap-3">
                  {filteredRows.map(({ id, model }) => (
                    <Field key={model} orientation="horizontal">
                      <Checkbox
                        id={id}
                        checked={selected.has(model)}
                        aria-label={`${t("Select")} ${model}`}
                        onCheckedChange={() => toggle(model)}
                      />
                      <FieldContent>
                        <FieldLabel htmlFor={id} className="font-mono">
                          {model}
                        </FieldLabel>
                      </FieldContent>
                    </Field>
                  ))}
                </FieldGroup>
              </FieldSet>
            ) : (
              <EmptyState
                title={t("No models match this search.")}
                description={t("Try a different model ID.")}
                className="min-h-48"
              />
            )}
          </ScrollArea>
        </div>

        <DialogFooter>
          <DialogClose asChild>
            <Button variant="outline">{t("Cancel")}</Button>
          </DialogClose>
          <Button onClick={apply}>{t("Apply selection")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
