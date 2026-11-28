import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Controller, useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  CalendarDays,
  Calculator,
  CheckCircle2,
  ChevronDown,
  Clock3,
  Plus,
  RotateCcw,
  Trash2,
  WandSparkles,
} from "lucide-react";
import { useParams, useSearchParams } from "react-router";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
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
import { Spinner } from "@/components/ui/spinner";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { DecimalField } from "@/components/shared/decimal-field";
import { DetailField } from "@/components/shared/detail-field";
import { ApiError } from "@/api/errors";
import type {
  BillingWeekday,
  ControlPlaneModel,
  ModelInput,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { safeAdminReturnPath } from "@/features/admin/model-setup/model-setup-navigation";
import { useModel, useUpdateModel } from "@/features/admin/api";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { useConfigurationDraft } from "@/hooks/use-configuration-draft";
import {
  multiplyDecimal,
} from "@/lib/decimal";
import { formatDateTime } from "@/lib/dates";
import { formatPricingTokens } from "./pricing-tokens";
import { cn } from "@/lib/utils";
import { ModelPriceComparison } from "./model-price-comparison";

import {
  ALL_WEEKDAYS,
  calculatorFormForModel,
  calculatorSchema,
  empty,
  emptyCalculator,
  fromLocalInput,
  PRICE_FIELDS,
  pricingFormForModel,
  sameWeekdays,
  schema,
  toLocalInput,
  WEEKDAY_LABELS,
  WEEKEND,
  WORKDAYS,
  type CalculatorForm,
  type PricingForm,
} from "./model-pricing-schema";
export function ModelPricingPage() {
  const { id = "" } = useParams();
  const [searchParams] = useSearchParams();
  const returnTo = safeAdminReturnPath(
    searchParams.get("returnTo"),
    `/admin/models/${id}`,
  );
  const { data, etag, isLoading, error, refetch } = useModel(id);
  const update = useUpdateModel(id);
  const { t } = useI18n();
  const pricingFormId = useId();
  const calculatorPanelId = useId();
  const pricingForm = useRef<HTMLFormElement>(null);
  const baselineEtag = useRef("");
  const [calculatorOpen, setCalculatorOpen] = useState(false);
  const [focusError, setFocusError] = useState(false);
  const [reloadRequired, setReloadRequired] = useState(false);
  const [isReloading, setIsReloading] = useState(false);
  const form = useForm<PricingForm>({
    resolver: zodResolver(schema),
    defaultValues: empty,
    shouldFocusError: false,
  });
  const calculator = useForm<CalculatorForm>({
    resolver: zodResolver(calculatorSchema),
    defaultValues: emptyCalculator,
  });
  const { reset: resetForm, formState: { isSubmitting } } = form;
  const { reset: resetCalculator } = calculator;
  const resetPricing = useCallback((model: ControlPlaneModel, version: string) => {
    resetForm(pricingFormForModel(model));
    resetCalculator(calculatorFormForModel(model));
    baselineEtag.current = version;
    setReloadRequired(false);
  }, [resetForm, resetCalculator]);
  const windows = useFieldArray({
    control: form.control,
    name: "time_multipliers",
  });
  const pricingIsDirty = form.formState.isDirty;
  const formIsDirty = pricingIsDirty || calculator.formState.isDirty;
  const { navigate, navigationGuard, markSaved } = useConfigurationDraft(isSubmitting, formIsDirty);

  useEffect(() => {
    if (!focusError || isSubmitting) return;
    const invalid = pricingForm.current?.querySelector<HTMLElement>(
      'input[aria-invalid="true"], [aria-invalid="true"][role="group"] button, [data-window-error]',
    );
    if (invalid) {
      invalid.focus({ preventScroll: true });
      invalid.scrollIntoView({ block: "center" });
    }
    setFocusError(false);
  }, [focusError, isSubmitting]);

  useEffect(() => {
    if (data && !formIsDirty) resetPricing(data.data, etag);
  }, [data, etag, resetPricing, formIsDirty]);

  const fillBasePrices = calculator.handleSubmit((values) => {
    for (const field of PRICE_FIELDS) {
      form.setValue(
        field.key,
        multiplyDecimal(values[field.referenceKey], values.calculator_multiplier),
        { shouldDirty: true, shouldValidate: true },
      );
    }
    toast.success(t("Calculated prices filled"), { position: "top-center" });
  });

  const copyBaseToReference = () => {
    const values = form.getValues();
    for (const field of PRICE_FIELDS) {
      calculator.setValue(field.referenceKey, values[field.key], {
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
        resetPricing(refreshed.data.data, refreshed.data.etag);
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
        resetPricing(refreshed.data.data, refreshed.data.etag);
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
          resetPricing(refreshed.data.data, refreshed.data.etag);
          toast.error(t("This pricing model was changed elsewhere. Reloading."));
        } else {
          setReloadRequired(true);
          toast.error(
            t("This pricing model changed elsewhere, but the latest version could not be reloaded."),
          );
        }
      } else {
        toast.error(mutationError instanceof Error ? mutationError.message : t("Save failed"));
      }
    }
  }, () => setFocusError(true));

  const timeWindowErrors = form.formState.errors.time_multipliers as
    | { message?: string; root?: { message?: string } }
    | undefined;
  const timeWindowCollectionError =
    timeWindowErrors?.message ?? timeWindowErrors?.root?.message;
  const formDisabled = isSubmitting || update.isPending || isReloading;
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
      navigationGuard={navigationGuard}
      title={data?.data.display_name || t("Model pricing")}
      backPath={returnTo}
      isLoading={isLoading}
      error={data ? null : error}
      hasData={Boolean(data)}
      editCard={
        data ? (
          <div className="flex min-w-0 flex-col gap-4">
          <ModelPriceComparison model={data.data} draft={form.watch()} />
          <div className="grid gap-3 [&_:is(input,button,[tabindex])]:scroll-my-24 lg:grid-cols-[minmax(0,2fr)_minmax(20rem,1fr)] lg:items-start">
            <form
              id={pricingFormId}
              ref={pricingForm}
              className="contents"
              noValidate
              onSubmit={submit}
            >
            <fieldset className="contents" disabled={formDisabled}>
            <Card variant="section" className="min-w-0 lg:col-start-1 lg:row-start-1">
              <CardHeader>
                <CardTitle>{t("Base prices")}</CardTitle>
                <CardDescription>
                  {t("Prices are in USD per token unit shown below.")}
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

            <Card variant="section" className="min-w-0 lg:col-start-1 lg:row-start-2">
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
                  <FieldError tabIndex={-1} data-window-error="">
                    {t(timeWindowCollectionError)}
                  </FieldError>
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
                          <CardTitle className="wrap-anywhere">{windowLabel}</CardTitle>
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

            </fieldset>
            </form>

            <div className="flex min-w-0 flex-col gap-6 lg:col-start-2 lg:row-span-2 lg:row-start-1">
              <Card variant="section">
                <CardHeader>
                  <CardTitle className="hidden lg:block"><h2>{t("Multiplier calculator")}</h2></CardTitle>
                  <Button
                    type="button"
                    variant="ghost"
                    className="justify-between lg:hidden"
                    aria-expanded={calculatorOpen}
                    aria-controls={calculatorPanelId}
                    onClick={() => setCalculatorOpen(!calculatorOpen)}
                  >
                    {t("Multiplier calculator")}
                    <ChevronDown data-icon="inline-end" className="group-aria-expanded/button:rotate-180" />
                  </Button>
                </CardHeader>
                <CardContent
                  id={calculatorPanelId}
                  className={cn(!calculatorOpen && "hidden lg:block")}
                >
                  <form onSubmit={fillBasePrices}>
                    <fieldset className="flex min-w-0 flex-col gap-5" disabled={formDisabled}>
                      <p className="text-sm text-muted-foreground">
                        {t(
                          "Multiply four reference prices and fill the base-price fields. Calculation changes stay local until you save.",
                        )}
                      </p>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="self-start"
                        onClick={copyBaseToReference}
                      >
                        {t("Copy base prices")}
                      </Button>
                      <FieldGroup>
                        {PRICE_FIELDS.map((field) => (
                          <Controller
                            key={field.referenceKey}
                            control={calculator.control}
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
                          control={calculator.control}
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
                      <Button type="submit" variant="secondary" className="h-auto min-h-8 whitespace-normal">
                        <Calculator data-icon="inline-start" />
                        {t("Calculate and fill base prices")}
                      </Button>
                    </fieldset>
                  </form>
                </CardContent>
              </Card>

              <Card variant="section">
                <CardHeader>
                  <CardTitle>{t("Review and save")}</CardTitle>
                </CardHeader>
                <CardContent>
                  <dl className="grid grid-cols-2 gap-4">
                    <DetailField label={t("Model")} value={data.data.source_model_id} mono />
                    <DetailField
                      label={t("Price unit tokens")}
                      value={formatPricingTokens(form.watch("price_unit_tokens"))}
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
              </Card>
            </div>

            <div className="bottom-[var(--console-dock-clearance,0px)] z-10 flex flex-wrap items-center justify-between gap-2 border-t bg-background py-2 pb-[max(--spacing(2),env(safe-area-inset-bottom))] lg:col-span-2 [@media(min-height:24rem)]:sticky">
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
              <div className="flex flex-wrap gap-2">
                <Button
                  type="submit"
                  form={pricingFormId}
                  disabled={formDisabled || reloadRequired || !pricingIsDirty}
                >
                  {isSubmitting || update.isPending ? (
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
                    disabled={formDisabled || !pricingIsDirty}
                    onClick={() => resetPricing(data.data, etag)}
                  >
                    <RotateCcw data-icon="inline-start" />
                    {t("Discard pricing changes")}
                  </Button>
                )}
              </div>
            </div>
          </div>
          </div>
        ) : null
      }
    />
  );
}
