import {Textarea} from "@/components/ui/textarea";
import {Field, FieldDescription, FieldError, FieldLabel} from "@/components/ui/field";

interface StringListFieldProps {
  label: string;
  description?: string;
  value: string[];
  onChange: (value: string[]) => void;
  placeholder?: string;
  error?: string;
  required?: boolean;
}

/**
 * Edits a string[] as one item per line. Trims and drops empty lines on blur.
 * Used for available_models, no_proxy_hosts, permissions, and similar arrays.
 */
export function StringListField({
  label,
  description,
  value,
  onChange,
  placeholder,
  error,
  required,
}: StringListFieldProps) {
  return (
    <Field data-invalid={Boolean(error)}>
      <FieldLabel>
        {label}
        {required ? <span className="text-destructive"> *</span> : null}
      </FieldLabel>
      <Textarea
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
