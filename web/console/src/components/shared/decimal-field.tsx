import { Input } from "@/components/ui/input";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";

interface DecimalFieldProps {
  label: string;
  value: string | null;
  onChange: (value: string) => void;
  error?: string;
  required?: boolean;
  description?: string;
}

/** Edits a rust_decimal string. Keeps the raw text so precision is preserved. */
export function DecimalField({
  label,
  value,
  onChange,
  error,
  required,
  description,
}: DecimalFieldProps) {
  return (
    <Field data-invalid={Boolean(error)}>
      <FieldLabel>
        {label}
        {required ? <span className="text-destructive"> *</span> : null}
      </FieldLabel>
      <Input
        inputMode="decimal"
        value={value ?? ""}
        placeholder="0"
        onChange={(event) => onChange(event.target.value)}
        aria-invalid={Boolean(error)}
      />
      {description ? (
        <p className="text-xs text-muted-foreground">{description}</p>
      ) : null}
      {error ? <FieldError>{error}</FieldError> : null}
    </Field>
  );
}

interface NullableNumberFieldProps {
  label: string;
  value: number | null;
  onChange: (value: number | null) => void;
  error?: string;
  description?: string;
}

/** Edits an optional integer; empty input maps to null (unset). */
export function NullableNumberField({
  label,
  value,
  onChange,
  error,
  description,
}: NullableNumberFieldProps) {
  return (
    <Field data-invalid={Boolean(error)}>
      <FieldLabel>{label}</FieldLabel>
      <Input
        type="number"
        value={value ?? ""}
        placeholder="unset"
        onChange={(event) => {
          const text = event.target.value.trim();
          if (text === "") {
            onChange(null);
            return;
          }
          const parsed = Number(text);
          onChange(Number.isFinite(parsed) ? Math.trunc(parsed) : null);
        }}
        aria-invalid={Boolean(error)}
      />
      {description ? (
        <p className="text-xs text-muted-foreground">{description}</p>
      ) : null}
      {error ? <FieldError>{error}</FieldError> : null}
    </Field>
  );
}
