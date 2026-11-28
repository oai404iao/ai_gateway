import { z } from "zod";
import type { SystemSettings } from "@/api/types";
import type { SettingsSection } from "./settings-sections";

function statusCodesAreValid(value: string): boolean {
  const codes = value
    .split(/[\s,]+/)
    .map((code) => code.trim())
    .filter(Boolean);
  const parsed = codes.map(Number);
  return (
    parsed.every((code) => Number.isInteger(code) && code >= 100 && code <= 599) &&
    new Set(parsed).size === parsed.length
  );
}

export function parseStatusCodes(value: string): number[] {
  return value
    .split(/[\s,]+/)
    .map((code) => code.trim())
    .filter(Boolean)
    .map(Number);
}

function retryStatusCodesAreValid(value: string): boolean {
  const codes = parseStatusCodes(value);
  return (
    codes.length <= 100 &&
    codes.every((code) => Number.isInteger(code) && code >= 400 && code <= 599) &&
    new Set(codes).size === codes.length
  );
}

const apiHostsSchema = z
  .array(
    z
      .string()
      .trim()
      .min(1, "API host cannot be blank.")
      .max(2048, "API host must be at most 2048 characters.")
      .refine(
        (value) => {
          try {
            const url = new URL(value);
            return (
              (url.protocol === "http:" || url.protocol === "https:") &&
              Boolean(url.hostname) &&
              !url.username &&
              !url.password &&
              !url.search &&
              !url.hash
            );
          } catch {
            return false;
          }
        },
        "Enter a valid HTTP(S) API host.",
      ),
  )
  .max(32, "Configure at most 32 API hosts.")
  .refine(
    (hosts) => {
      const normalizedHosts = hosts.map((host) => {
        try {
          return new URL(host).toString();
        } catch {
          return host;
        }
      });
      return new Set(normalizedHosts).size === hosts.length;
    },
    "API hosts must be unique.",
  );

const upstreamSchema = z
  .object({
    connect_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
    response_header_timeout_seconds: z
      .number()
      .int()
      .min(1, "Enter a positive number of seconds."),
    images_response_header_timeout_seconds: z
      .number()
      .int()
      .min(1, "Enter a positive number of seconds."),
    standalone_web_search_response_header_timeout_seconds: z
      .number()
      .int()
      .min(1, "Enter a positive number of seconds."),
    stream_idle_timeout_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
  })
  .superRefine((upstream, context) => {
    if (upstream.response_header_timeout_seconds <= upstream.connect_timeout_seconds) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["response_header_timeout_seconds"],
        message: "Response header timeout must exceed connect timeout.",
      });
    }
    if (upstream.images_response_header_timeout_seconds <= upstream.connect_timeout_seconds) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["images_response_header_timeout_seconds"],
        message: "Images response header timeout must exceed connect timeout.",
      });
    }
    if (
      upstream.standalone_web_search_response_header_timeout_seconds <=
      upstream.connect_timeout_seconds
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["standalone_web_search_response_header_timeout_seconds"],
        message: "Web search response header timeout must exceed connect timeout.",
      });
    }
  });

const requestRetrySchema = z.object({
  enabled: z.boolean(),
  max_retries: z
    .number()
    .int()
    .min(1, "Maximum retries must be between 1 and 10.")
    .max(10, "Maximum retries must be between 1 and 10."),
  retryable_status_codes: z
    .string()
    .refine(
      retryStatusCodesAreValid,
      "Enter at most 100 unique HTTP status codes from 400 through 599, separated by commas.",
    ),
});

const passiveHealthSchema = z.object({
  connection_failure_threshold: z
    .number()
    .int()
    .min(1, "Enter a positive failure threshold."),
  cooldown_seconds: z.number().int().min(1, "Enter a positive number of seconds."),
});

const automaticDisableSchema = z.object({
  enabled: z.boolean(),
  error_status_codes: z
    .string()
    .refine(
      statusCodesAreValid,
      "Enter unique HTTP status codes from 100 through 599, separated by commas.",
    ),
  error_message_keywords: z
    .array(z.string().trim().min(1, "Keyword cannot be blank.").max(200))
    .refine(
      (keywords) => new Set(keywords.map((keyword) => keyword.toLocaleLowerCase())).size === keywords.length,
      "Error keywords must be unique.",
    ),
});

const scheduledTestingSchema = z.object({
  mode: z.enum(["global", "failure_only"]),
  auto_recover: z.boolean(),
  interval_minutes: z.number().int().min(1, "Enter a positive number of minutes."),
  prompt: z.string().trim().min(1, "Test prompt is required.").max(4000),
});

const sessionAffinitySchema = z.object({
  enabled: z.boolean(),
  max_entries: z
    .number()
    .int()
    .min(1, "Enter a positive cache capacity.")
    .max(1_000_000, "Cache capacity cannot exceed 1000000 entries."),
  default_ttl_seconds: z
    .number()
    .int()
    .min(1, "Enter a positive number of seconds.")
    .max(604_800, "Affinity TTL cannot exceed 604800 seconds."),
  rules: z
    .array(
      z.object({
        name: z.string().trim().min(1).max(64),
        enabled: z.boolean(),
        api_formats: z
          .array(z.enum(["open_ai_chat_completions", "open_ai_responses"]))
          .min(1),
        model_regex: z.array(z.string().min(1).max(256)).max(8),
        key_sources: z
          .array(
            z.discriminatedUnion("type", [
              z.object({
                type: z.literal("request_header"),
                name: z.string().trim().min(1).max(256),
              }),
              z.object({
                type: z.literal("json_pointer"),
                pointer: z.string().trim().startsWith("/").max(256),
              }),
            ]),
          )
          .min(1)
          .max(8),
        value_regex: z.string().min(1).max(256).nullable(),
        ttl_seconds: z.number().int().min(1).max(604_800).nullable(),
      }),
    )
    .max(64)
    .refine(
      (rules) =>
        new Set(rules.map((rule) => rule.name.trim().toLocaleLowerCase())).size ===
        rules.length,
      "Rule names must be unique.",
    ),
});

const websocketSchema = z
  .object({
    enabled: z.boolean(),
    max_idle_connections: z
      .number()
      .int()
      .min(0, "Idle pool capacity must be between 0 and 4096.")
      .max(4096, "Idle pool capacity must be between 0 and 4096."),
    idle_timeout_seconds: z
      .number()
      .int()
      .min(1, "WebSocket idle timeout must be between 1 and 3600 seconds.")
      .max(3600, "WebSocket idle timeout must be between 1 and 3600 seconds."),
    max_connection_age_seconds: z
      .number()
      .int()
      .min(60, "Maximum WebSocket age must be between 60 and 3600 seconds.")
      .max(3600, "Maximum WebSocket age must be between 60 and 3600 seconds."),
  })
  .superRefine((websocket, context) => {
    if (websocket.idle_timeout_seconds >= websocket.max_connection_age_seconds) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["max_connection_age_seconds"],
        message: "Maximum WebSocket age must exceed the idle timeout.",
      });
    }
  });

const systemSettingsSchema = z.object({
  api_hosts: apiHostsSchema,
  upstream: upstreamSchema,
  request_retry: requestRetrySchema,
  passive_health: passiveHealthSchema,
  automatic_disable: automaticDisableSchema,
  scheduled_testing: scheduledTestingSchema,
  session_affinity: sessionAffinitySchema,
  websocket: websocketSchema,
});

const passthroughSystemShape = {
  api_hosts: z.any(),
  upstream: z.any(),
  request_retry: z.any(),
  passive_health: z.any(),
  automatic_disable: z.any(),
  scheduled_testing: z.any(),
  session_affinity: z.any(),
  websocket: z.any(),
};

export function sectionSettingsSchema(section: SettingsSection) {
  switch (section) {
    case "general":
      return z.object({ ...passthroughSystemShape, api_hosts: apiHostsSchema });
    case "upstream":
      return z.object({ ...passthroughSystemShape, upstream: upstreamSchema });
    case "reliability":
      return z.object({
        ...passthroughSystemShape,
        request_retry: requestRetrySchema,
        passive_health: passiveHealthSchema,
        automatic_disable: automaticDisableSchema,
      });
    case "testing":
      return z.object({ ...passthroughSystemShape, scheduled_testing: scheduledTestingSchema });
    case "websocket":
      return z.object({ ...passthroughSystemShape, websocket: websocketSchema });
    case "affinity":
      return z.object({ ...passthroughSystemShape, session_affinity: sessionAffinitySchema });
    case "maintenance":
      return z.object(passthroughSystemShape);
  }
}

export type SystemSettingsValues = z.infer<typeof systemSettingsSchema>;


export type SystemSettingsVersion = { data: SystemSettings; etag: string };

export const defaultValues: SystemSettingsValues = {
  api_hosts: [],
  upstream: {
    connect_timeout_seconds: 10,
    response_header_timeout_seconds: 30,
    images_response_header_timeout_seconds: 300,
    standalone_web_search_response_header_timeout_seconds: 300,
    stream_idle_timeout_seconds: 90,
  },
  request_retry: {
    enabled: true,
    max_retries: 1,
    retryable_status_codes: "",
  },
  passive_health: {
    connection_failure_threshold: 3,
    cooldown_seconds: 30,
  },
  automatic_disable: {
    enabled: false,
    error_status_codes: "",
    error_message_keywords: [],
  },
  scheduled_testing: {
    mode: "global",
    auto_recover: true,
    interval_minutes: 5,
    prompt: "reply '1'",
  },
  session_affinity: {
    enabled: false,
    max_entries: 100_000,
    default_ttl_seconds: 3_600,
    rules: [],
  },
  websocket: {
    enabled: false,
    max_idle_connections: 128,
    idle_timeout_seconds: 300,
    max_connection_age_seconds: 3300,
  },
};

