import { useState, type KeyboardEvent, type ReactNode } from "react";
import { PlusIcon, XIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/app/i18n";

interface StringListFieldProps {
  id?: string;
  className?: string;
  label: string;
  description?: string;
  value: string[];
  onChange: (value: string[]) => void;
  placeholder?: string;
  error?: string;
  required?: boolean;
  variant?: "lines" | "tokens";
  addLabel?: string;
  action?: ReactNode;
}

/**
 * Edits a string[] as one item per line or as removable tokens.
 * Used for available_models, no_proxy_hosts, permissions, and similar arrays.
 */
export function StringListField({
  id,
  className,
  label,
  description,
  value,
  onChange,
  placeholder,
  error,
  required,
  variant = "lines",
  addLabel,
  action,
}: StringListFieldProps) {
  const { t } = useI18n();
  const [draft, setDraft] = useState("");
  const nextItem = draft.trim();
  const fieldLabel = (
    <FieldLabel htmlFor={id}>
      {label}
      {required ? <span className="text-destructive"> *</span> : null}
    </FieldLabel>
  );

  const add = () => {
    if (!nextItem || value.includes(nextItem)) return;
    onChange([...value, nextItem]);
    setDraft("");
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    add();
  };

  if (variant === "tokens") {
    return (
      <Field className={className} data-invalid={Boolean(error)}>
        {action ? (
          <div className="flex flex-wrap items-center justify-between gap-2">
            {fieldLabel}
            {action}
          </div>
        ) : (
          fieldLabel
        )}
        {value.length > 0 ? (
          <div className="flex flex-wrap items-center gap-1.5">
            {value.map((item, index) => (
              <div key={`${item}-${index}`} className="flex items-center gap-1">
                <Badge variant="secondary">{item}</Badge>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label={t("Remove {item}", { item })}
                  onClick={() => onChange(value.filter((_, itemIndex) => itemIndex !== index))}
                >
                  <XIcon data-icon="inline-start" />
                </Button>
              </div>
            ))}
          </div>
        ) : null}
        <InputGroup>
          <InputGroupInput
            id={id}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder ?? t("Enter an item")}
            aria-invalid={Boolean(error)}
          />
          <InputGroupAddon align="inline-end">
            <InputGroupButton
              disabled={!nextItem || value.includes(nextItem)}
              onClick={add}
            >
              <PlusIcon data-icon="inline-start" />
              {addLabel ?? t("Add")}
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        {description ? <FieldDescription>{description}</FieldDescription> : null}
        {error ? <FieldError>{error}</FieldError> : null}
      </Field>
    );
  }

  return (
    <Field className={className} data-invalid={Boolean(error)}>
      {action ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          {fieldLabel}
          {action}
        </div>
      ) : (
        fieldLabel
      )}
      <Textarea
        id={id}
        rows={3}
        value={value.join("\n")}
        placeholder={placeholder ?? t("One item per line")}
        onChange={(event) => onChange(event.target.value.split("\n"))}
        onBlur={() => onChange(value.map((item) => item.trim()).filter(Boolean))}
        aria-invalid={Boolean(error)}
      />
      {description ? <FieldDescription>{description}</FieldDescription> : null}
      {error ? <FieldError>{error}</FieldError> : null}
    </Field>
  );
}
