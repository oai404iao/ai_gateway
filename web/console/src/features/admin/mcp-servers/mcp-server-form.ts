import { z } from "zod";
import type {
  ApiFormat,
  ImageMcpSettings,
  McpImageBackground,
  McpImageQuality,
  McpSearchContextSize,
  McpSearchExternalWebAccess,
  McpServerCreateInput,
  McpServerInput,
  McpServerKind,
  McpServerSettings,
  McpServerView,
  WebSearchMcpSettings,
} from "@/api/types";

const MCP_SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{0,62}$/;
const DOMAIN_PATTERN =
  /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)*[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i;

const domainListSchema = z.array(z.string()).superRefine((values, context) => {
  const normalized = normalizeDomains(values);
  if (normalized.length > 100) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      message: "Each domain list supports at most 100 entries.",
    });
  }
  const seen = new Set<string>();
  for (const domain of normalized) {
    if (domain.length > 253 || !DOMAIN_PATTERN.test(domain)) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "Enter valid DNS domain names without schemes, paths, or wildcards.",
      });
      return;
    }
    if (seen.has(domain)) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message: "Domain lists cannot contain duplicate entries.",
      });
      return;
    }
    seen.add(domain);
  }
});

export const mcpServerFormSchema = z
  .object({
    slug: z
      .string()
      .trim()
      .regex(
        MCP_SLUG_PATTERN,
        "Use 1-63 lowercase letters, digits, or hyphens, starting with a letter or digit.",
      ),
    kind: z.enum(["web_search", "image"]),
    name: z.string().trim().min(1, "Name is required.").max(100),
    description: z.string().max(1_000),
    model_rule_id: z.string().min(1, "Pick a compatible model rule."),
    enabled: z.boolean(),
    external_web_access: z.enum(["cached", "indexed", "live"]),
    search_context_size: z.enum(["low", "medium", "high"]),
    allowed_domains: domainListSchema,
    blocked_domains: domainListSchema,
    max_output_tokens_short: z
      .number()
      .int("Token limits must be whole numbers.")
      .min(1, "Token limits must be at least 1.")
      .max(100_000, "Token limits must not exceed 100000."),
    max_output_tokens_medium: z
      .number()
      .int("Token limits must be whole numbers.")
      .min(1, "Token limits must be at least 1.")
      .max(100_000, "Token limits must not exceed 100000."),
    max_output_tokens_long: z
      .number()
      .int("Token limits must be whole numbers.")
      .min(1, "Token limits must be at least 1.")
      .max(100_000, "Token limits must not exceed 100000."),
    image_background: z.enum(["auto", "opaque", "transparent"]),
    image_quality: z.enum(["auto", "low", "medium", "high"]),
    image_size: z.string().trim(),
  })
  .superRefine((value, context) => {
    if (value.kind === "web_search") {
      if (
        value.max_output_tokens_short > value.max_output_tokens_medium ||
        value.max_output_tokens_medium > value.max_output_tokens_long
      ) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["max_output_tokens_short"],
          message: "Token limits must be ordered as short ≤ medium ≤ long.",
        });
      }
      const allowed = new Set(normalizeDomains(value.allowed_domains));
      if (
        normalizeDomains(value.blocked_domains).some((domain) =>
          allowed.has(domain),
        )
      ) {
        context.addIssue({
          code: z.ZodIssueCode.custom,
          path: ["blocked_domains"],
          message: "A domain cannot appear in both allowed and blocked lists.",
        });
      }
    }

    if (value.kind === "image" && !isValidImageSize(value.image_size)) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["image_size"],
        message:
          "Use auto or WIDTHxHEIGHT with canonical dimensions from 64 to 8192.",
      });
    }
  });

export type McpServerFormValues = z.infer<typeof mcpServerFormSchema>;

export const defaultMcpServerFormValues: McpServerFormValues = {
  slug: "",
  kind: "web_search",
  name: "",
  description: "",
  model_rule_id: "",
  enabled: true,
  external_web_access: "live",
  search_context_size: "medium",
  allowed_domains: [],
  blocked_domains: [],
  max_output_tokens_short: 1_000,
  max_output_tokens_medium: 3_000,
  max_output_tokens_long: 6_000,
  image_background: "auto",
  image_quality: "auto",
  image_size: "auto",
};

export function mcpServerFormValues(
  server: McpServerView,
): McpServerFormValues {
  const values = {
    ...defaultMcpServerFormValues,
    slug: server.slug,
    kind: server.kind,
    name: server.name,
    description: server.description ?? "",
    model_rule_id: server.model_rule_id,
    enabled: server.enabled,
  };
  if (server.kind === "web_search") {
    const settings = server.settings as WebSearchMcpSettings;
    return {
      ...values,
      external_web_access: settings.external_web_access ?? "live",
      search_context_size: settings.search_context_size ?? "medium",
      allowed_domains: settings.allowed_domains ?? [],
      blocked_domains: settings.blocked_domains ?? [],
      max_output_tokens_short: settings.max_output_tokens?.short ?? 1_000,
      max_output_tokens_medium: settings.max_output_tokens?.medium ?? 3_000,
      max_output_tokens_long: settings.max_output_tokens?.long ?? 6_000,
    };
  }
  const settings = server.settings as ImageMcpSettings;
  return {
    ...values,
    image_background: settings.background ?? "auto",
    image_quality: settings.quality ?? "auto",
    image_size: settings.size ?? "auto",
  };
}

export function mcpServerCreateInput(
  values: McpServerFormValues,
): McpServerCreateInput {
  return {
    slug: values.slug.trim(),
    kind: values.kind,
    name: values.name.trim(),
    description: optionalDescription(values.description),
    model_rule_id: values.model_rule_id,
    settings: mcpServerSettings(values),
    enabled: values.enabled,
  };
}

export function mcpServerInput(
  values: McpServerFormValues,
): McpServerInput {
  return {
    name: values.name.trim(),
    description: optionalDescription(values.description),
    model_rule_id: values.model_rule_id,
    settings: mcpServerSettings(values),
    enabled: values.enabled,
  };
}

export function mcpServerSettings(
  values: McpServerFormValues,
): McpServerSettings {
  if (values.kind === "web_search") {
    return {
      external_web_access: values.external_web_access,
      search_context_size: values.search_context_size,
      allowed_domains: normalizeDomains(values.allowed_domains),
      blocked_domains: normalizeDomains(values.blocked_domains),
      max_output_tokens: {
        short: values.max_output_tokens_short,
        medium: values.max_output_tokens_medium,
        long: values.max_output_tokens_long,
      },
    } satisfies WebSearchMcpSettings;
  }
  return {
    background: values.image_background,
    quality: values.image_quality,
    size: values.image_size.trim().toLowerCase(),
  } satisfies ImageMcpSettings;
}

export function normalizeDomains(values: string[]): string[] {
  return values
    .map((value) => value.trim().toLowerCase())
    .filter(Boolean);
}

export function mcpKindApiFormat(kind: McpServerKind): ApiFormat {
  return kind === "web_search" ? "open_ai_responses" : "open_ai_images";
}

export function mcpKindLabel(kind: McpServerKind): string {
  return kind === "web_search" ? "Web search" : "Images";
}

export function mcpToolName(kind: McpServerKind): string {
  return kind === "web_search" ? "web.run" : "image_gen.imagegen";
}

export function mcpExternalAccessLabel(
  value: McpSearchExternalWebAccess,
): string {
  switch (value) {
    case "cached":
      return "Cached";
    case "indexed":
      return "Indexed";
    case "live":
      return "Live";
  }
}

export function mcpContextSizeLabel(
  value: McpSearchContextSize,
): string {
  switch (value) {
    case "low":
      return "Low";
    case "medium":
      return "Medium";
    case "high":
      return "High";
  }
}

export function mcpImageBackgroundLabel(
  value: McpImageBackground,
): string {
  switch (value) {
    case "auto":
      return "Auto";
    case "opaque":
      return "Opaque";
    case "transparent":
      return "Transparent";
  }
}

export function mcpImageQualityLabel(value: McpImageQuality): string {
  switch (value) {
    case "auto":
      return "Auto";
    case "low":
      return "Low";
    case "medium":
      return "Medium";
    case "high":
      return "High";
  }
}

function optionalDescription(value: string): string | null {
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function isValidImageSize(value: string): boolean {
  const normalized = value.trim().toLowerCase();
  if (normalized === "auto") return true;
  const match = /^([1-9][0-9]{1,4})x([1-9][0-9]{1,4})$/.exec(normalized);
  if (!match) return false;
  const width = Number(match[1]);
  const height = Number(match[2]);
  return (
    Number.isInteger(width) &&
    Number.isInteger(height) &&
    width >= 64 &&
    width <= 8_192 &&
    height >= 64 &&
    height <= 8_192
  );
}
