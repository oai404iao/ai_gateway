import { useId } from "react";
import { Input } from "@/components/ui/input";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
} from "@/components/ui/field";
import { useI18n } from "@/app/i18n";

interface DecimalFieldProps {
  id?: string;
  label: string;
  value: string | null;
  onChange: (value: string) => void;
  error?: string;
  required?: boolean;
  description?: string;
}

/** Edits a rust_decimal string. Keeps the raw text so precision is preserved. */
export function DecimalField({
  id,
  label,
  value,
  onChange,
  error,
  required,
  description,
}: DecimalFieldProps) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  return (
    <Field data-invalid={Boolean(error)}>
      <FieldLabel htmlFor={inputId}>
        {label}
        {required ? <span className="text-destructive"> *</span> : null}
      </FieldLabel>
      <Input
        id={inputId}
        inputMode="decimal"
        value={value ?? ""}
        placeholder="0"
        onChange={(event) => onChange(event.target.value)}
        aria-invalid={Boolean(error)}
      />
      {description ? <FieldDescription>{description}</FieldDescription> : null}
      {error ? <FieldError>{error}</FieldError> : null}
    </Field>
  );
}

interface NullableNumberFieldProps {
  id?: string;
  label: string;
  value: number | null;
  onChange: (value: number | null) => void;
  error?: string;
  description?: string;
}

/** Edits an optional integer; empty input maps to null (unset). */
export function NullableNumberField({
  id,
  label,
  value,
  onChange,
  error,
  description,
}: NullableNumberFieldProps) {
  const { t } = useI18n();
  const generatedId = useId();
  const inputId = id ?? generatedId;
  return (
    <Field data-invalid={Boolean(error)}>
      <FieldLabel htmlFor={inputId}>{label}</FieldLabel>
      <Input
        id={inputId}
        type="number"
        value={value ?? ""}
        placeholder={t("unset")}
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
      {description ? <FieldDescription>{description}</FieldDescription> : null}
      {error ? <FieldError>{error}</FieldError> : null}
    </Field>
  );
}
