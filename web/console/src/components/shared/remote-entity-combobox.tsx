import { useMemo, useState } from "react";
import { useI18n } from "@/app/i18n";
import {
  Combobox,
  ComboboxCollection,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";

export interface RemoteEntityOption {
  id: string;
  label: string;
}

interface RemoteEntityComboboxProps {
  id?: string;
  ariaLabel?: string;
  allLabel: string;
  value: string;
  selectedLabel?: string;
  onValueChange: (value: string) => void;
  options: RemoteEntityOption[];
  isLoading?: boolean;
  isError?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  search: string;
  onSearchChange: (search: string) => void;
  disabled?: boolean;
}

const ALL_VALUE = "__all__";

export function RemoteEntityCombobox({
  id,
  ariaLabel,
  allLabel,
  value,
  selectedLabel,
  onValueChange,
  options,
  isLoading = false,
  isError = false,
  open,
  onOpenChange,
  search,
  onSearchChange,
  disabled = false,
}: RemoteEntityComboboxProps) {
  const { t } = useI18n();
  const [cachedLabels, setCachedLabels] = useState<Map<string, string>>(() => new Map());

  const labelById = useMemo(() => {
    const map = new Map(cachedLabels);
    map.set(ALL_VALUE, allLabel);
    for (const option of options) {
      map.set(option.id, option.label);
    }
    if (value && selectedLabel) {
      map.set(value, selectedLabel);
    }
    return map;
  }, [allLabel, cachedLabels, options, selectedLabel, value]);

  const itemIds = useMemo(
    () => [ALL_VALUE, ...options.map((option) => option.id)],
    [options],
  );

  const selectedDisplay = value ? (labelById.get(value) ?? t("Unavailable")) : "";
  const inputValue = open ? search : selectedDisplay;

  return (
    <Combobox
      items={itemIds}
      value={value || null}
      inputValue={inputValue}
      filter={null}
      open={open}
      disabled={disabled}
      itemToStringLabel={(itemId) =>
        itemId === ALL_VALUE ? allLabel : (labelById.get(itemId) ?? t("Unavailable"))
      }
      onOpenChange={(nextOpen) => {
        onOpenChange(nextOpen);
        if (!nextOpen) onSearchChange("");
      }}
      onInputValueChange={(nextInput, details) => {
        if (details.reason === "input-change") {
          onSearchChange(nextInput);
        } else if (details.reason === "clear-press") {
          onSearchChange("");
          onValueChange("");
        }
      }}
      onValueChange={(nextId) => {
        if (!nextId || nextId === ALL_VALUE) {
          onValueChange("");
          onSearchChange("");
          return;
        }
        const label = labelById.get(nextId);
        if (label) {
          setCachedLabels((prev) => {
            if (prev.get(nextId) === label) return prev;
            const next = new Map(prev);
            next.set(nextId, label);
            return next;
          });
        }
        onValueChange(nextId);
        onSearchChange("");
      }}
    >
      <ComboboxInput
        id={id}
        aria-label={ariaLabel}
        placeholder={allLabel}
        disabled={disabled}
        spellCheck={false}
      />
      <ComboboxContent>
        <ComboboxEmpty>
          {isLoading
            ? t("Loading…")
            : isError
              ? t("Unavailable")
              : t("No matching suggestions")}
        </ComboboxEmpty>
        <ComboboxList>
          <ComboboxGroup>
            <ComboboxCollection>
              {(itemId: string) => (
                <ComboboxItem key={itemId} value={itemId}>
                  <span className="truncate">
                    {itemId === ALL_VALUE ? allLabel : (labelById.get(itemId) ?? itemId)}
                  </span>
                </ComboboxItem>
              )}
            </ComboboxCollection>
          </ComboboxGroup>
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  );
}
