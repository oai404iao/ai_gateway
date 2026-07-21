import { useState, type KeyboardEvent } from "react";
import { PlusIcon, XIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

interface StringListFieldProps {
  id?: string;
  label: string;
  description?: string;
  value: string[];
  onChange: (value: string[]) => void;
  placeholder?: string;
  error?: string;
  required?: boolean;
  variant?: "lines" | "tokens";
  addLabel?: string;
}

/**
 * Edits a string[] as one item per line or as removable tokens.
 * Used for available_models, no_proxy_hosts, permissions, and similar arrays.
 */
export function StringListField({
  id,
  label,
  description,
  value,
  onChange,
  placeholder,
  error,
  required,
  variant = "lines",
  addLabel = "Add",
}: StringListFieldProps) {
  const [draft, setDraft] = useState("");
  const nextItem = draft.trim();

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
      <Field data-invalid={Boolean(error)}>
        <FieldLabel htmlFor={id}>
          {label}
          {required ? <span className="text-destructive"> *</span> : null}
        </FieldLabel>
        {value.length > 0 ? (
          <div className="flex flex-wrap items-center gap-1.5">
            {value.map((item, index) => (
              <div key={`${item}-${index}`} className="flex items-center gap-1">
                <Badge variant="secondary">{item}</Badge>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Remove ${item}`}
                  onClick={() => onChange(value.filter((_, itemIndex) => itemIndex !== index))}
                >
                  <XIcon data-icon="inline-start" />
                </Button>
              </div>
            ))}
          </div>
        ) : null}
        <div className="flex gap-2">
          <Input
            id={id}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder ?? "Enter an item"}
            aria-invalid={Boolean(error)}
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!nextItem || value.includes(nextItem)}
            onClick={add}
          >
            <PlusIcon data-icon="inline-start" />
            {addLabel}
          </Button>
        </div>
        {description ? <FieldDescription>{description}</FieldDescription> : null}
        {error ? <FieldError>{error}</FieldError> : null}
      </Field>
    );
  }

  return (
    <Field data-invalid={Boolean(error)}>
      <FieldLabel htmlFor={id}>
        {label}
        {required ? <span className="text-destructive"> *</span> : null}
      </FieldLabel>
      <Textarea
        id={id}
        rows={3}
        value={value.join("\n")}
        placeholder={placeholder ?? "One item per line"}
        onChange={(event) => onChange(event.target.value.split("\n"))}
        onBlur={() => onChange(value.map((item) => item.trim()).filter(Boolean))}
        aria-invalid={Boolean(error)}
      />
      {description ? <FieldDescription>{description}</FieldDescription> : null}
      {error ? <FieldError>{error}</FieldError> : null}
    </Field>
  );
}
