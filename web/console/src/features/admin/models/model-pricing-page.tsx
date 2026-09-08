import { useEffect, useRef, useState } from "react";
import { Controller, useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  CalendarDays,
  Calculator,
  CheckCircle2,
  Clock3,
  Plus,
  RotateCcw,
  Trash2,
  WandSparkles,
} from "lucide-react";
import { useParams, useSearchParams } from "react-router";
import { toast } from "sonner";
import { z } from "zod";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { DecimalField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { ApiError } from "@/api/errors";
import type {
  BillingWeekday,
  ControlPlaneModel,
  ModelInput,
  TimeBillingMultiplier,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { safeAdminReturnPath } from "@/features/admin/model-setup/model-setup-navigation";
import { useModel, useUpdateModel } from "@/features/admin/api";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";
import {
  isNonNegativeDecimal,
  isNonNegativeRustDecimal,
  multiplyDecimal,
} from "@/lib/decimal";
import { formatDateTime } from "@/lib/dates";

const UTC_TIME_PATTERN = /^(?:[01][0-9]|2[0-3]):[0-5][0-9]$/;
const MINUTES_PER_DAY = 24 * 60;
const MINUTES_PER_WEEK = 7 * MINUTES_PER_DAY;

const ALL_WEEKDAYS = [
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
  "sunday",
] as const satisfies readonly BillingWeekday[];
const WORKDAYS = ALL_WEEKDAYS.slice(0, 5);
const WEEKEND = ALL_WEEKDAYS.slice(5);
const WEEKDAY_LABELS: Record<BillingWeekday, { short: string; long: string }> = {
  monday: { short: "Mon", long: "Monday" },
  tuesday: { short: "Tue", long: "Tuesday" },
  wednesday: { short: "Wed", long: "Wednesday" },
  thursday: { short: "Thu", long: "Thursday" },
  friday: { short: "Fri", long: "Friday" },
  saturday: { short: "Sat", long: "Saturday" },
  sunday: { short: "Sun", long: "Sunday" },
};

const PRICE_FIELDS = [
  {
    key: "input_unit_price",
    referenceKey: "reference_input_unit_price",
    label: "Input unit price",
  },
  {
    key: "cached_input_unit_price",
    referenceKey: "reference_cached_input_unit_price",
    label: "Cached input unit price",
  },
  {
    key: "cache_write_unit_price",
    referenceKey: "reference_cache_write_unit_price",
    label: "Cache write unit price",
  },
  {
    key: "output_unit_price",
    referenceKey: "reference_output_unit_price",
    label: "Output unit price",
  },
] as const;

function isStoredPrice(value: string): boolean {
  if (!isNonNegativeDecimal(value)) return false;
  const [whole, fraction = ""] = value.split(".");
  return whole.length <= 12 && fraction.length <= 12;
}

function minuteOfDay(value: string): number {
  const [hours, minutes] = value.split(":").map(Number);
  return hours * 60 + minutes;
}

function windowDuration(start: number, end: number): number {
  return start < end ? end - start : MINUTES_PER_DAY - start + end;
}

function normalizeWindow(
  window: TimeBillingMultiplier,
): TimeBillingMultiplier & { weekdays: BillingWeekday[] } {
  return {
    ...window,
    weekdays: window.weekdays ?? [...ALL_WEEKDAYS],
  };
}

function pricingFormForModel(model: ControlPlaneModel): PricingForm {
  return {
    price_unit_tokens: model.price_unit_tokens,
    input_unit_price: model.input_unit_price,
    cached_input_unit_price: model.cached_input_unit_price,
    cache_write_unit_price: model.cache_write_unit_price,
    output_unit_price: model.output_unit_price,
    price_effective_at: toLocalInput(model.price_effective_at),
    reference_input_unit_price: model.input_unit_price,
    reference_cached_input_unit_price: model.cached_input_unit_price,
    reference_cache_write_unit_price: model.cache_write_unit_price,
    reference_output_unit_price: model.output_unit_price,
    calculator_multiplier: "1",
    time_multipliers: (model.advanced_billing.time_multipliers ?? []).map(normalizeWindow),
  };
}

function sameWeekdays(
  value: readonly BillingWeekday[],
  expected: readonly BillingWeekday[],
): boolean {
  return value.length === expected.length && expected.every((weekday) => value.includes(weekday));
}

const decimalMessage = "Enter a non-negative decimal with at most 12 decimal places.";
const decimalSchema = z.string().refine(isStoredPrice, decimalMessage);
const multiplierSchema = z
  .string()
  .refine(
    isNonNegativeRustDecimal,
    "Enter a non-negative multiplier within the supported decimal range.",
  );
const timeWindowSchema = z.object({
  label: z
    .string()
    .min(1, "Window label is required.")
    .max(80, "Window label is too long.")
    .refine((value) => value.trim() === value, "Window label cannot start or end with spaces."),
  weekdays: z
    .array(z.enum(ALL_WEEKDAYS))
    .min(1, "Select at least one UTC weekday.")
    .max(7)
    .refine(
      (weekdays) => new Set(weekdays).size === weekdays.length,
      "UTC weekdays cannot repeat.",
    ),
  start_time: z.string().regex(UTC_TIME_PATTERN, "Enter a UTC time in HH:MM format."),
  end_time: z.string().regex(UTC_TIME_PATTERN, "Enter a UTC time in HH:MM format."),
  multiplier: multiplierSchema,
});

const schema = z
  .object({
    price_unit_tokens: z
      .number({ error: "Price unit tokens must be a positive integer." })
      .int("Price unit tokens must be a positive integer.")
      .positive("Price unit tokens must be a positive integer."),
    input_unit_price: decimalSchema,
    cached_input_unit_price: decimalSchema,
    cache_write_unit_price: decimalSchema,
    output_unit_price: decimalSchema,
    price_effective_at: z.string().min(1, "Price effective time is required."),
    reference_input_unit_price: decimalSchema,
    reference_cached_input_unit_price: decimalSchema,
    reference_cache_write_unit_price: decimalSchema,
    reference_output_unit_price: decimalSchema,
    calculator_multiplier: multiplierSchema,
    time_multipliers: z
      .array(timeWindowSchema)
      .max(32, "A model can have at most 32 UTC price windows."),
  })
  .superRefine((value, context) => {
    const labels = new Set<string>();
    const coveredBy = new Array<number | undefined>(MINUTES_PER_WEEK);
    value.time_multipliers.forEach((window, index) => {
      if (labels.has(window.label)) {
        context.addIssue({
          code: "custom",
          path: ["time_multipliers", index, "label"],
          message: "Window labels must be unique.",
        });
      }
      labels.add(window.label);
      if (
        !UTC_TIME_PATTERN.test(window.start_time) ||
        !UTC_TIME_PATTERN.test(window.end_time) ||
        window.weekdays.length === 0
      ) {
        return;
      }
      const start = minuteOfDay(window.start_time);
      const end = minuteOfDay(window.end_time);
      if (start === end) {
        context.addIssue({
          code: "custom",
          path: ["time_multipliers", index, "end_time"],
          message: "Start and end times must differ.",
        });
        return;
      }
      const duration = windowDuration(start, end);
      for (const weekday of new Set(window.weekdays)) {
        const startOfWindow = ALL_WEEKDAYS.indexOf(weekday) * MINUTES_PER_DAY + start;
        for (let offset = 0; offset < duration; offset += 1) {
          const weekMinute = (startOfWindow + offset) % MINUTES_PER_WEEK;
          if (coveredBy[weekMinute] !== undefined) {
            context.addIssue({
              code: "custom",
              path: ["time_multipliers", index, "start_time"],
              message: "UTC price windows cannot overlap on the selected weekdays.",
            });
            return;
          }
          coveredBy[weekMinute] = index;
        }
      }
    });
  });

type PricingForm = z.infer<typeof schema>;

const empty: PricingForm = {
  price_unit_tokens: 1_000_000,
  input_unit_price: "0",
  cached_input_unit_price: "0",
  cache_write_unit_price: "0",
  output_unit_price: "0",
  price_effective_at: "",
  reference_input_unit_price: "0",
  reference_cached_input_unit_price: "0",
  reference_cache_write_unit_price: "0",
  reference_output_unit_price: "0",
  calculator_multiplier: "1",
  time_multipliers: [],
};

function toLocalInput(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number) => String(value).padStart(2, "0");
  const milliseconds = String(date.getMilliseconds()).padStart(3, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${milliseconds}`;
}

function fromLocalInput(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toISOString();
}

export function ModelPricingPage() {
  const { id = "" } = useParams();
  const [searchParams] = useSearchParams();
  const returnTo = safeAdminReturnPath(
    searchParams.get("returnTo"),
    `/admin/models/${id}`,
  );
  const returnsToSetup = returnTo.startsWith("/admin/model-setup");
  const { data, etag, isLoading, error, refetch } = useModel(id);
  const update = useUpdateModel(id);
  const { t } = useI18n();
  const baselineEtag = useRef("");
  const [reloadRequired, setReloadRequired] = useState(false);
  const [isReloading, setIsReloading] = useState(false);
  const form = useForm<PricingForm>({
    resolver: zodResolver(schema),
    defaultValues: empty,
  });
  const windows = useFieldArray({
    control: form.control,
    name: "time_multipliers",
  });
  const formIsDirty = form.formState.isDirty;
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(form.formState.isSubmitting, formIsDirty);

  useEffect(() => {
    if (!data || formIsDirty) return;
    form.reset(pricingFormForModel(data.data));
    baselineEtag.current = etag;
    setReloadRequired(false);
  }, [data, etag, form, formIsDirty]);

  const fillBasePrices = () => {
    const values = form.getValues();
    try {
      for (const field of PRICE_FIELDS) {
        form.setValue(
          field.key,
          multiplyDecimal(values[field.referenceKey], values.calculator_multiplier),
          { shouldDirty: true, shouldValidate: true },
        );
      }
      toast.success(t("Calculated prices filled"));
    } catch {
      toast.error(t("Reference prices and multiplier must be valid non-negative decimals."));
    }
  };

  const copyBaseToReference = () => {
    const values = form.getValues();
    for (const field of PRICE_FIELDS) {
      form.setValue(field.referenceKey, values[field.key], {
        shouldDirty: true,
        shouldValidate: true,
      });
    }
  };

  const applyDeepSeekPreset = () => {
    windows.replace([
      {
        label: t("DeepSeek peak 1"),
        weekdays: [...ALL_WEEKDAYS],
        start_time: "01:00",
        end_time: "04:00",
        multiplier: "2",
      },
      {
        label: t("DeepSeek peak 2"),
        weekdays: [...ALL_WEEKDAYS],
        start_time: "06:00",
        end_time: "10:00",
        multiplier: "2",
      },
    ]);
    toast.success(t("DeepSeek UTC peak windows filled"));
  };

  const addWindow = () => {
    windows.append({
      label: t("Price window {number}", { number: windows.fields.length + 1 }),
      weekdays: [...ALL_WEEKDAYS],
      start_time: "00:00",
      end_time: "01:00",
      multiplier: "1",
    });
  };

  const reloadLatest = async () => {
    setIsReloading(true);
    try {
      const refreshed = await refetch();
      if (refreshed.isSuccess && refreshed.data) {
        baselineEtag.current = refreshed.data.etag;
        form.reset(pricingFormForModel(refreshed.data.data));
        setReloadRequired(false);
        toast.success(t("Latest model pricing loaded"));
      } else {
        toast.error(t("The latest model pricing could not be reloaded."));
      }
    } finally {
      setIsReloading(false);
    }
  };

  const submit = form.handleSubmit(async (values) => {
    if (!data || reloadRequired) return;
    const model = data.data;
    const input: ModelInput = {
      source_model_id: model.source_model_id,
      display_name: model.display_name,
      provider_name: model.provider_name,
      enabled: model.enabled,
      price_unit_tokens: values.price_unit_tokens,
      input_unit_price: values.input_unit_price,
      cached_input_unit_price: values.cached_input_unit_price,
      cache_write_unit_price: values.cache_write_unit_price,
      output_unit_price: values.output_unit_price,
      price_effective_at:
        values.price_effective_at === toLocalInput(model.price_effective_at)
          ? model.price_effective_at
          : fromLocalInput(values.price_effective_at),
      advanced_billing: {
        long_context_tiers: model.advanced_billing.long_context_tiers,
        request_multipliers: model.advanced_billing.request_multipliers,
        time_multipliers: values.time_multipliers,
      },
    };
    try {
      await update.mutateAsync({ input, ifMatch: baselineEtag.current });
      const refreshed = await refetch();
      if (refreshed.isSuccess && refreshed.data) {
        baselineEtag.current = refreshed.data.etag;
        form.reset(pricingFormForModel(refreshed.data.data));
        setReloadRequired(false);
        toast.success(t("Model pricing updated"));
        markSaved();
        if (searchParams.has("returnTo")) navigate(returnTo, { replace: true });
      } else {
        setReloadRequired(true);
        toast.error(
          t("Pricing was saved, but the latest model version could not be reloaded."),
        );
      }
    } catch (mutationError) {
      if (mutationError instanceof ApiError && mutationError.isConflict) {
        const refreshed = await refetch();
        if (refreshed.isSuccess && refreshed.data) {
          baselineEtag.current = refreshed.data.etag;
          form.reset(pricingFormForModel(refreshed.data.data));
          setReloadRequired(false);
          toast.error(t("This upstream model was changed elsewhere. Reloading."));
        } else {
          setReloadRequired(true);
          toast.error(
            t("This upstream model changed elsewhere, but the latest version could not be reloaded."),
          );
        }
      } else {
        toast.error(mutationError instanceof Error ? mutationError.message : t("Save failed"));
      }
    }
  });

  const timeWindowErrors = form.formState.errors.time_multipliers as
    | { message?: string; root?: { message?: string } }
    | undefined;
  const timeWindowCollectionError =
    timeWindowErrors?.message ?? timeWindowErrors?.root?.message;
  const dirtyFields = form.formState.dirtyFields;
  const formDisabled = form.formState.isSubmitting || isReloading;
  const pricingIsDirty = Boolean(
    dirtyFields.price_unit_tokens ||
      dirtyFields.input_unit_price ||
      dirtyFields.cached_input_unit_price ||
      dirtyFields.cache_write_unit_price ||
      dirtyFields.output_unit_price ||
      dirtyFields.price_effective_at ||
      dirtyFields.time_multipliers,
  );
  const watchedWindows = form.watch("time_multipliers");
  const configuredWeekdays = ALL_WEEKDAYS.filter((weekday) =>
    watchedWindows.some((window) => window.weekdays.includes(weekday)),
  );
  const weekdaySummary =
    configuredWeekdays.length === 0
      ? t("None")
      : sameWeekdays(configuredWeekdays, ALL_WEEKDAYS)
        ? t("Every day")
        : sameWeekdays(configuredWeekdays, WORKDAYS)
          ? t("Weekdays")
          : sameWeekdays(configuredWeekdays, WEEKEND)
            ? t("Weekend")
            : configuredWeekdays.map((weekday) => t(WEEKDAY_LABELS[weekday].short)).join(", ");

  return (
    <AdminDetailShell
      configurationLens="models"
      navigationGuard={navigationGuard}
      saving={form.formState.isSubmitting}
      onBack={() => navigate(returnTo)}
      title={data?.data.display_name || t("Model pricing")}
      description={t("Configure base USD prices and weekly UTC peak or off-peak multipliers.")}
      backPath={returnTo}
      backLabel={t(
        returnsToSetup ? "Back to model setup" : "Back to upstream model",
      )}
      isLoading={isLoading}
      error={data ? null : error}
      hasData={Boolean(data)}
      editCard={
        data ? (
          <form
            className="grid gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(20rem,1fr)] lg:items-start"
            onSubmit={submit}
          >
            <fieldset className="contents" disabled={formDisabled}>
            <Card className="min-w-0 lg:col-start-1 lg:row-start-1">
              <CardHeader>
                <CardTitle>{t("Base prices")}</CardTitle>
                <CardDescription>
                  {t("USD prices are per the configured price unit tokens.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <FieldGroup className="grid gap-5 md:grid-cols-2">
                  <Field data-invalid={Boolean(form.formState.errors.price_unit_tokens)}>
                    <FieldLabel htmlFor="pricing_price_unit_tokens">
                      {t("Price unit tokens")}
                    </FieldLabel>
                    <Input
                      id="pricing_price_unit_tokens"
                      type="number"
                      min={1}
                      aria-invalid={Boolean(form.formState.errors.price_unit_tokens)}
                      {...form.register("price_unit_tokens", { valueAsNumber: true })}
                    />
                    {form.formState.errors.price_unit_tokens ? (
                      <FieldError>
                        {t(form.formState.errors.price_unit_tokens.message ?? "")}
                      </FieldError>
                    ) : null}
                  </Field>
                  <Field data-invalid={Boolean(form.formState.errors.price_effective_at)}>
                    <FieldLabel htmlFor="pricing_price_effective_at">
                      {t("Price effective at")}
                    </FieldLabel>
                    <Input
                      id="pricing_price_effective_at"
                      type="datetime-local"
                      step={0.001}
                      aria-invalid={Boolean(form.formState.errors.price_effective_at)}
                      {...form.register("price_effective_at")}
                    />
                    {form.formState.errors.price_effective_at ? (
                      <FieldError>
                        {t(form.formState.errors.price_effective_at.message ?? "")}
                      </FieldError>
                    ) : null}
                  </Field>
                  {PRICE_FIELDS.map((field) => (
                    <Controller
                      key={field.key}
                      control={form.control}
                      name={field.key}
                      render={({ field: controlled, fieldState }) => (
                        <DecimalField
                          label={t(field.label)}
                          value={controlled.value}
                          onChange={controlled.onChange}
                          error={fieldState.error ? t(fieldState.error.message ?? "") : undefined}
                          required
                        />
                      )}
                    />
                  ))}
                </FieldGroup>
              </CardContent>
            </Card>

            <Card className="min-w-0 lg:col-start-1 lg:row-start-2">
              <CardHeader>
                <CardTitle>{t("UTC peak and off-peak windows")}</CardTitle>
                <CardDescription>
                  {t(
                    "Windows repeat on selected UTC weekdays. Outside all windows the multiplier is 1. Start is inclusive and end is exclusive.",
                  )}
                </CardDescription>
                <div className="mt-2 flex flex-wrap gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={applyDeepSeekPreset}>
                    <WandSparkles data-icon="inline-start" />
                    {t("Use DeepSeek preset")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={windows.fields.length >= 32}
                    onClick={addWindow}
                  >
                    <Plus data-icon="inline-start" />
                    {t("Add window")}
                  </Button>
                </div>
                {timeWindowCollectionError ? (
                  <FieldError>{t(timeWindowCollectionError)}</FieldError>
                ) : null}
              </CardHeader>
              <CardContent className="flex flex-col gap-4">
                <Alert>
                  <Clock3 data-icon="inline-start" />
                  <AlertTitle>{t("DeepSeek reference")}</AlertTitle>
                  <AlertDescription>
                    {t(
                      "The preset treats stored base prices as off-peak prices and applies 2× during 01:00–04:00 and 06:00–10:00 UTC.",
                    )}
                  </AlertDescription>
                </Alert>
                {windows.fields.length === 0 ? (
                  <Empty>
                    <EmptyHeader>
                      <EmptyMedia variant="icon">
                        <CalendarDays />
                      </EmptyMedia>
                      <EmptyTitle>{t("No time-based pricing")}</EmptyTitle>
                      <EmptyDescription>
                        {t("The base prices apply all week with multiplier 1.")}
                      </EmptyDescription>
                    </EmptyHeader>
                    <EmptyContent>
                      <Button
                        type="button"
                        variant="outline"
                        disabled={windows.fields.length >= 32}
                        onClick={addWindow}
                      >
                        <Plus data-icon="inline-start" />
                        {t("Add first price window")}
                      </Button>
                    </EmptyContent>
                  </Empty>
                ) : (
                  windows.fields.map((window, index) => {
                    const errors = form.formState.errors.time_multipliers?.[index];
                    const selectedWeekdays = form.watch(
                      `time_multipliers.${index}.weekdays`,
                    );
                    const selectedWeekdaySummary = sameWeekdays(
                      selectedWeekdays,
                      ALL_WEEKDAYS,
                    )
                      ? t("Every day")
                      : sameWeekdays(selectedWeekdays, WORKDAYS)
                        ? t("Weekdays")
                        : sameWeekdays(selectedWeekdays, WEEKEND)
                          ? t("Weekend")
                          : selectedWeekdays
                              .map((weekday) => t(WEEKDAY_LABELS[weekday].short))
                              .join(", ");
                    const windowLabel =
                      form.watch(`time_multipliers.${index}.label`) ||
                      t("Price window {number}", { number: index + 1 });
                    return (
                      <Card key={window.id} size="sm">
                        <CardHeader>
                          <CardTitle>{windowLabel}</CardTitle>
                          <CardDescription>{t("Weekly UTC price window")}</CardDescription>
                          <CardAction className="flex items-center gap-2">
                            <Badge variant="secondary">{selectedWeekdaySummary}</Badge>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Remove {label}", { label: windowLabel })}
                              onClick={() => windows.remove(index)}
                            >
                              <Trash2 />
                            </Button>
                          </CardAction>
                        </CardHeader>
                        <CardContent className="flex flex-col gap-5">
                          <FieldGroup className="grid gap-5 md:grid-cols-2">
                            <Field data-invalid={Boolean(errors?.label)}>
                              <FieldLabel htmlFor={`time_window_${index}_label`}>
                                {t("Label")}
                              </FieldLabel>
                              <Input
                                id={`time_window_${index}_label`}
                                aria-invalid={Boolean(errors?.label)}
                                {...form.register(`time_multipliers.${index}.label`)}
                              />
                              {errors?.label ? (
                                <FieldError>{t(errors.label.message ?? "")}</FieldError>
                              ) : null}
                            </Field>
                            <Controller
                              control={form.control}
                              name={`time_multipliers.${index}.multiplier`}
                              render={({ field, fieldState }) => (
                                <DecimalField
                                  label={t("Window multiplier")}
                                  value={field.value}
                                  onChange={field.onChange}
                                  error={
                                    fieldState.error
                                      ? t(fieldState.error.message ?? "")
                                      : undefined
                                  }
                                  required
                                />
                              )}
                            />
                            <Field data-invalid={Boolean(errors?.start_time)}>
                              <FieldLabel htmlFor={`time_window_${index}_start`}>
                                {t("Start time (UTC)")}
                              </FieldLabel>
                              <Input
                                id={`time_window_${index}_start`}
                                type="time"
                                step={60}
                                aria-invalid={Boolean(errors?.start_time)}
                                {...form.register(`time_multipliers.${index}.start_time`)}
                              />
                              {errors?.start_time ? (
                                <FieldError>{t(errors.start_time.message ?? "")}</FieldError>
                              ) : null}
                            </Field>
                            <Field data-invalid={Boolean(errors?.end_time)}>
                              <FieldLabel htmlFor={`time_window_${index}_end`}>
                                {t("End time (UTC)")}
                              </FieldLabel>
                              <Input
                                id={`time_window_${index}_end`}
                                type="time"
                                step={60}
                                aria-invalid={Boolean(errors?.end_time)}
                                {...form.register(`time_multipliers.${index}.end_time`)}
                              />
                              {errors?.end_time ? (
                                <FieldError>{t(errors.end_time.message ?? "")}</FieldError>
                              ) : null}
                            </Field>
                          </FieldGroup>
                          <Controller
                            control={form.control}
                            name={`time_multipliers.${index}.weekdays`}
                            render={({ field, fieldState }) => (
                              <FieldSet data-invalid={Boolean(fieldState.error)}>
                                <FieldLegend variant="label">
                                  {t("Applicable UTC weekdays")}
                                </FieldLegend>
                                <FieldDescription>
                                  {t(
                                    "For an overnight window, weekdays identify the UTC day on which the window starts.",
                                  )}
                                </FieldDescription>
                                <ToggleGroup
                                  multiple
                                  variant="outline"
                                  size="sm"
                                  value={field.value}
                                  onValueChange={(value) =>
                                    field.onChange(value as BillingWeekday[])
                                  }
                                  aria-label={t("Applicable UTC weekdays")}
                                  aria-invalid={Boolean(fieldState.error)}
                                  className="flex-wrap"
                                >
                                  {ALL_WEEKDAYS.map((weekday) => (
                                    <ToggleGroupItem
                                      key={weekday}
                                      value={weekday}
                                      aria-label={t(WEEKDAY_LABELS[weekday].long)}
                                    >
                                      {t(WEEKDAY_LABELS[weekday].short)}
                                    </ToggleGroupItem>
                                  ))}
                                </ToggleGroup>
                                <div className="flex flex-wrap gap-1">
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="xs"
                                    onClick={() => field.onChange([...ALL_WEEKDAYS])}
                                  >
                                    {t("Every day")}
                                  </Button>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="xs"
                                    onClick={() => field.onChange([...WORKDAYS])}
                                  >
                                    {t("Weekdays")}
                                  </Button>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="xs"
                                    onClick={() => field.onChange([...WEEKEND])}
                                  >
                                    {t("Weekend")}
                                  </Button>
                                </div>
                                {fieldState.error ? (
                                  <FieldError>
                                    {t(fieldState.error.message ?? "")}
                                  </FieldError>
                                ) : null}
                              </FieldSet>
                            )}
                          />
                          <FieldDescription>
                            {t(
                              "This multiplier applies uniformly to input, cached input, cache write, and output prices.",
                            )}
                          </FieldDescription>
                        </CardContent>
                      </Card>
                    );
                  })
                )}
              </CardContent>
            </Card>

            <Card className="min-w-0 lg:sticky lg:top-6 lg:col-start-2 lg:row-span-2 lg:row-start-1">
              <CardHeader>
                <CardTitle>{t("Multiplier calculator")}</CardTitle>
                <CardDescription>
                  {t(
                    "Multiply four reference prices and fill the base-price fields. Calculation changes stay local until you save.",
                  )}
                </CardDescription>
                <CardAction>
                  <Button type="button" variant="outline" size="sm" onClick={copyBaseToReference}>
                    {t("Copy base prices")}
                  </Button>
                </CardAction>
              </CardHeader>
              <CardContent className="flex flex-col gap-5">
                <FieldGroup>
                  {PRICE_FIELDS.map((field) => (
                    <Controller
                      key={field.referenceKey}
                      control={form.control}
                      name={field.referenceKey}
                      render={({ field: controlled, fieldState }) => (
                        <DecimalField
                          label={t(`Reference ${field.label.toLowerCase()}`)}
                          value={controlled.value}
                          onChange={controlled.onChange}
                          error={fieldState.error ? t(fieldState.error.message ?? "") : undefined}
                          required
                        />
                      )}
                    />
                  ))}
                  <Controller
                    control={form.control}
                    name="calculator_multiplier"
                    render={({ field, fieldState }) => (
                      <DecimalField
                        label={t("Calculator multiplier")}
                        value={field.value}
                        onChange={field.onChange}
                        error={fieldState.error ? t(fieldState.error.message ?? "") : undefined}
                        description={t("For example, 1.2 adds a 20% markup and 0.5 halves prices.")}
                        required
                      />
                    )}
                  />
                </FieldGroup>
                <Button type="button" variant="secondary" onClick={fillBasePrices}>
                  <Calculator data-icon="inline-start" />
                  {t("Calculate and fill base prices")}
                </Button>
                <Separator />
                <div className="flex items-center justify-between gap-3">
                  <div className="flex flex-col gap-1">
                    <h2 className="text-sm font-medium">{t("Review and save")}</h2>
                    <p className="text-xs text-muted-foreground">
                      {t("Confirm the effective price unit and weekly schedule before saving.")}
                    </p>
                  </div>
                  <Badge
                    variant={pricingIsDirty || reloadRequired ? "default" : "secondary"}
                    aria-live="polite"
                  >
                    {reloadRequired
                      ? t("Reload required")
                      : pricingIsDirty
                        ? t("Unsaved changes")
                        : t("Saved")}
                  </Badge>
                </div>
                <dl className="grid grid-cols-2 gap-4">
                  <DetailField label={t("Model")} value={data.data.source_model_id} mono />
                  <DetailField
                    label={t("Price unit tokens")}
                    value={form.watch("price_unit_tokens").toLocaleString()}
                  />
                  <DetailField label={t("Price windows")} value={watchedWindows.length} />
                  <DetailField label={t("Window start weekdays")} value={weekdaySummary} />
                  <DetailField
                    label={t("Effective")}
                    value={formatDateTime(fromLocalInput(form.watch("price_effective_at")))}
                    className="col-span-2"
                  />
                </dl>
              </CardContent>
              <CardFooter className="flex-col items-stretch gap-2">
                <Button
                  type="submit"
                  disabled={
                    form.formState.isSubmitting ||
                    update.isPending ||
                    reloadRequired ||
                    !pricingIsDirty
                  }
                >
                  {form.formState.isSubmitting || update.isPending ? (
                    <Spinner data-icon="inline-start" />
                  ) : (
                    <CheckCircle2 data-icon="inline-start" />
                  )}
                  {t("Save model pricing")}
                </Button>
                {reloadRequired ? (
                  <Button
                    type="button"
                    variant="secondary"
                    disabled={isReloading}
                    onClick={reloadLatest}
                  >
                    {isReloading ? (
                      <Spinner data-icon="inline-start" />
                    ) : (
                      <RotateCcw data-icon="inline-start" />
                    )}
                    {t("Reload latest model")}
                  </Button>
                ) : (
                  <Button
                    type="button"
                    variant="ghost"
                    disabled={
                      !pricingIsDirty ||
                      form.formState.isSubmitting ||
                      update.isPending
                    }
                    onClick={() => {
                      baselineEtag.current = etag;
                      form.reset(pricingFormForModel(data.data));
                    }}
                  >
                    <RotateCcw data-icon="inline-start" />
                    {t("Discard pricing changes")}
                  </Button>
                )}
              </CardFooter>
            </Card>
            </fieldset>
          </form>
        ) : null
      }
    />
  );
}
