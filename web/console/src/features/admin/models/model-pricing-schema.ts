import { z } from "zod";
import type { BillingWeekday, ControlPlaneModel, TimeBillingMultiplier } from "@/api/types";
import { isNonNegativeDecimal, isNonNegativeRustDecimal } from "@/lib/decimal";

const UTC_TIME_PATTERN = /^(?:[01][0-9]|2[0-3]):[0-5][0-9]$/;
const MINUTES_PER_DAY = 24 * 60;
const MINUTES_PER_WEEK = 7 * MINUTES_PER_DAY;

export const ALL_WEEKDAYS = [
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday",
  "sunday",
] as const satisfies readonly BillingWeekday[];
export const WORKDAYS = ALL_WEEKDAYS.slice(0, 5);
export const WEEKEND = ALL_WEEKDAYS.slice(5);
export const WEEKDAY_LABELS: Record<BillingWeekday, { short: string; long: string }> = {
  monday: { short: "Mon", long: "Monday" },
  tuesday: { short: "Tue", long: "Tuesday" },
  wednesday: { short: "Wed", long: "Wednesday" },
  thursday: { short: "Thu", long: "Thursday" },
  friday: { short: "Fri", long: "Friday" },
  saturday: { short: "Sat", long: "Saturday" },
  sunday: { short: "Sun", long: "Sunday" },
};

export const PRICE_FIELDS = [
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

export function minuteOfDay(value: string): number {
  const [hours, minutes] = value.split(":").map(Number);
  return hours * 60 + minutes;
}

export function windowDuration(start: number, end: number): number {
  return start < end ? end - start : MINUTES_PER_DAY - start + end;
}

export function normalizeWindow(
  window: TimeBillingMultiplier,
): TimeBillingMultiplier & { weekdays: BillingWeekday[] } {
  return {
    ...window,
    weekdays: window.weekdays ?? [...ALL_WEEKDAYS],
  };
}

export function pricingFormForModel(model: ControlPlaneModel): PricingForm {
  return {
    price_unit_tokens: model.price_unit_tokens,
    input_unit_price: model.input_unit_price,
    cached_input_unit_price: model.cached_input_unit_price,
    cache_write_unit_price: model.cache_write_unit_price,
    output_unit_price: model.output_unit_price,
    price_effective_at: toLocalInput(model.price_effective_at),
    time_multipliers: (model.advanced_billing.time_multipliers ?? []).map(normalizeWindow),
  };
}

export function calculatorFormForModel(model: ControlPlaneModel): CalculatorForm {
  return {
    reference_input_unit_price: model.input_unit_price,
    reference_cached_input_unit_price: model.cached_input_unit_price,
    reference_cache_write_unit_price: model.cache_write_unit_price,
    reference_output_unit_price: model.output_unit_price,
    calculator_multiplier: "1",
  };
}

export function sameWeekdays(
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

export const schema = z
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

export type PricingForm = z.infer<typeof schema>;

export const calculatorSchema = z.object({
  reference_input_unit_price: decimalSchema,
  reference_cached_input_unit_price: decimalSchema,
  reference_cache_write_unit_price: decimalSchema,
  reference_output_unit_price: decimalSchema,
  calculator_multiplier: multiplierSchema,
});

export type CalculatorForm = z.infer<typeof calculatorSchema>;

export const empty: PricingForm = {
  price_unit_tokens: 1_000_000,
  input_unit_price: "0",
  cached_input_unit_price: "0",
  cache_write_unit_price: "0",
  output_unit_price: "0",
  price_effective_at: "",
  time_multipliers: [],
};

export const emptyCalculator: CalculatorForm = {
  reference_input_unit_price: "0",
  reference_cached_input_unit_price: "0",
  reference_cache_write_unit_price: "0",
  reference_output_unit_price: "0",
  calculator_multiplier: "1",
};

export function toLocalInput(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number) => String(value).padStart(2, "0");
  const milliseconds = String(date.getMilliseconds()).padStart(3, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${milliseconds}`;
}

export function fromLocalInput(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toISOString();
}

