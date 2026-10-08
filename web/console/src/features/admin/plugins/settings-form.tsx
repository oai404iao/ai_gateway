import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useI18n } from "@/app/i18n";
import type { PluginLocalizedText, PluginSettingsView, PluginSettingsValues } from "@/api/types";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

function localized(text: PluginLocalizedText | undefined, locale: string): string {
  return text?.[locale] ?? text?.en ?? Object.values(text ?? {})[0] ?? "";
}

export function PluginSettingsForm({ settings, saving, onSave }: {
  settings: PluginSettingsView;
  saving: boolean;
  onSave: (values: PluginSettingsValues) => void;
}) {
  const { t, locale } = useI18n();
  const schema = z.record(z.string(), z.union([z.string(), z.number(), z.boolean()]).optional())
    .superRefine((values, context) => {
      for (const field of settings.descriptor.fields) {
        const value = values[field.key];
        if (value === undefined) {
          if (field.required) context.addIssue({ code: "custom", path: [field.key], message: t("This setting is required.") });
          continue;
        }
        const valid = field.type === "string" ? typeof value === "string" && Array.from(value).length <= field.max_length && !/\p{Cc}/u.test(value)
          : field.type === "boolean" ? typeof value === "boolean"
          : field.type === "integer" ? typeof value === "number" && Number.isSafeInteger(value) && value >= field.minimum && value <= field.maximum
          : typeof value === "string" && field.options.some((option) => option.value === value);
        if (!valid) context.addIssue({ code: "custom", path: [field.key], message: t("Invalid setting value.") });
      }
    });
  const form = useForm<z.infer<typeof schema>>({
    resolver: zodResolver(schema),
    defaultValues: Object.fromEntries(settings.descriptor.fields.map((field) => [field.key, settings.values[field.key]])),
  });
  const draft = useConfigurationDraft(saving, form.formState.isDirty);
  const submit = form.handleSubmit((values) => {
    const entries = settings.descriptor.fields.flatMap((field) => {
      const value = values[field.key];
      return value === undefined ? [] : [[field.key, value]];
    });
    onSave(Object.fromEntries(entries) as PluginSettingsValues);
  });
  return (
    <form onSubmit={(event) => void submit(event)}>
      {draft.navigationGuard}
      <Card>
        <CardHeader>
          <CardTitle>{localized(settings.descriptor.title, locale) || t("Plugin settings")}</CardTitle>
          <CardDescription>{t("Settings are defined and validated by this plugin. Do not enter credentials here.")}</CardDescription>
        </CardHeader>
        <CardContent>
          <FieldGroup>
            {settings.descriptor.fields.map((field) => {
              const id = `plugin-setting-${field.key}`;
              const error = form.formState.errors[field.key];
              const label = localized(field.label, locale) || field.key;
              const description = localized(field.description, locale);
              return (
                <Field key={field.key} data-invalid={Boolean(error)} data-disabled={saving}>
                  <FieldLabel htmlFor={id}>{label}</FieldLabel>
                  {field.type === "boolean" ? (
                    <Switch id={id} checked={form.watch(field.key) === true}
                      onCheckedChange={(value) => form.setValue(field.key, value, { shouldDirty: true })}
                      disabled={saving} aria-invalid={Boolean(error)} />
                  ) : field.type === "enum" ? (
                    <Select value={String(form.watch(field.key) ?? "")}
                      onValueChange={(value) => form.setValue(field.key, value ?? undefined, { shouldDirty: true })}
                      disabled={saving}>
                      <SelectTrigger id={id} aria-invalid={Boolean(error)}>
                        <SelectValue>{field.options.find((option) => option.value === form.watch(field.key))?.label
                          ? localized(field.options.find((option) => option.value === form.watch(field.key))!.label, locale)
                          : t("Select an option")}</SelectValue>
                      </SelectTrigger>
                      <SelectContent><SelectGroup>
                        {field.options.map((option) => <SelectItem key={option.value} value={option.value}>{localized(option.label, locale) || option.value}</SelectItem>)}
                      </SelectGroup></SelectContent>
                    </Select>
                  ) : (
                    <Input id={id} type={field.type === "integer" ? "number" : "text"}
                      min={field.type === "integer" ? field.minimum : undefined}
                      max={field.type === "integer" ? field.maximum : undefined}
                      step={field.type === "integer" ? 1 : undefined}
                      aria-invalid={Boolean(error)} disabled={saving}
                      {...form.register(field.key, field.type === "integer"
                        ? { setValueAs: (value: string) => value === "" ? undefined : Number(value) } : {})} />
                  )}
                  {description ? <FieldDescription>{description}</FieldDescription> : null}
                  {field.type === "integer" ? <FieldDescription>{t("Allowed range: {minimum}–{maximum}", { minimum: field.minimum, maximum: field.maximum })}</FieldDescription> : null}
                  {error?.message ? <FieldError>{error.message}</FieldError> : null}
                </Field>
              );
            })}
          </FieldGroup>
        </CardContent>
        <CardFooter>
          <Button type="submit" disabled={saving || settings.descriptor.fields.length === 0}>{t("Save plugin settings")}</Button>
        </CardFooter>
      </Card>
    </form>
  );
}
