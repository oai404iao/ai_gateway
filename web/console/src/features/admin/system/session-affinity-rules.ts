import type {
  SystemSessionAffinityKeySource,
  SystemSessionAffinityRule,
  SystemSessionAffinitySettings,
} from "@/api/types";

export interface SessionAffinityErrors {
  maxEntries?: string;
  defaultTtl?: string;
  rules?: string;
}

export interface SessionAffinityCardProps {
  value: SystemSessionAffinitySettings;
  disabled?: boolean;
  onChange: (value: SystemSessionAffinitySettings) => void;
  errors?: SessionAffinityErrors;
}

export const EMPTY_RULE: SystemSessionAffinityRule = {
  name: "",
  enabled: true,
  api_formats: ["open_ai_responses"],
  model_regex: [],
  key_sources: [{ type: "json_pointer", pointer: "/prompt_cache_key" }],
  value_regex: null,
  ttl_seconds: null,
};

const SENSITIVE_HEADERS = new Set([
  "authorization",
  "proxy-authorization",
  "proxy-authenticate",
  "cookie",
  "set-cookie",
  "host",
  "content-length",
  "connection",
  "transfer-encoding",
  "keep-alive",
  "te",
  "trailer",
  "upgrade",
  "proxy-connection",
]);
const HEADER_NAME_PATTERN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

export function cloneRule(rule: SystemSessionAffinityRule): SystemSessionAffinityRule {
  return {
    ...rule,
    api_formats: [...rule.api_formats],
    model_regex: [...rule.model_regex],
    key_sources: rule.key_sources.map((source) => ({ ...source })),
  };
}

export function sourceValue(source: SystemSessionAffinityKeySource): string {
  return source.type === "request_header" ? source.name : source.pointer;
}

export function sourceWithValue(
  type: SystemSessionAffinityKeySource["type"],
  value: string,
): SystemSessionAffinityKeySource {
  return type === "request_header"
    ? { type, name: value }
    : { type, pointer: value };
}

export function uniqueRuleName(rules: SystemSessionAffinityRule[], base: string): string {
  const names = new Set(rules.map((rule) => rule.name.toLocaleLowerCase()));
  if (!names.has(base.toLocaleLowerCase())) return base;
  for (let index = 2; index < 1_000; index += 1) {
    const candidate = `${base}-${index}`;
    if (!names.has(candidate.toLocaleLowerCase())) return candidate;
  }
  return `${base}-${Date.now()}`;
}

export function codexRule(rules: SystemSessionAffinityRule[]): SystemSessionAffinityRule {
  return {
    name: uniqueRuleName(rules, "codex-responses"),
    enabled: true,
    api_formats: ["open_ai_responses"],
    model_regex: ["^gpt-.*$"],
    key_sources: [
      { type: "json_pointer", pointer: "/prompt_cache_key" },
      { type: "json_pointer", pointer: "/id" },
      { type: "request_header", name: "session-id" },
      { type: "request_header", name: "thread-id" },
    ],
    value_regex: null,
    ttl_seconds: null,
  };
}

export function moveItem<T>(items: T[], index: number, offset: number): T[] {
  const target = index + offset;
  if (target < 0 || target >= items.length) return items;
  const next = [...items];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

function validJsonPointer(pointer: string): boolean {
  if (!pointer.startsWith("/")) return false;
  for (let index = 0; index < pointer.length; index += 1) {
    if (pointer[index] !== "~") continue;
    index += 1;
    if (index >= pointer.length || (pointer[index] !== "0" && pointer[index] !== "1")) {
      return false;
    }
  }
  return true;
}

export function validateRule(
  rule: SystemSessionAffinityRule,
  existing: SystemSessionAffinityRule[],
  editingIndex: number | null,
): string | null {
  const name = rule.name.trim();
  if (!name) return "Rule name is required.";
  if (name.length > 64) return "Rule name must be at most 64 characters.";
  if (
    existing.some(
      (candidate, index) =>
        index !== editingIndex &&
        candidate.name.trim().toLocaleLowerCase() === name.toLocaleLowerCase(),
    )
  ) {
    return "Rule names must be unique.";
  }
  if (rule.api_formats.length === 0) return "Pick at least one format.";
  if (rule.key_sources.length === 0) return "Add at least one key source.";
  if (rule.key_sources.length > 8) return "A rule can contain at most 8 key sources.";
  if (rule.model_regex.length > 8) return "A rule can contain at most 8 model expressions.";
  if (rule.model_regex.some((pattern) => !pattern || pattern.length > 256)) {
    return "Model regular expressions must contain 1 through 256 characters.";
  }
  if (rule.value_regex !== null && (!rule.value_regex || rule.value_regex.length > 256)) {
    return "Value regular expression must contain 1 through 256 characters.";
  }
  if (
    rule.ttl_seconds !== null &&
    (!Number.isInteger(rule.ttl_seconds) || rule.ttl_seconds < 1 || rule.ttl_seconds > 604_800)
  ) {
    return "Rule TTL must be between 1 and 604800 seconds.";
  }
  for (const source of rule.key_sources) {
    const value = sourceValue(source).trim();
    if (!value || value.length > 256) return "Every key source must contain a value.";
    if (source.type === "json_pointer" && !validJsonPointer(value)) {
      return "Enter a valid RFC 6901 JSON pointer.";
    }
    if (source.type === "request_header") {
      if (!HEADER_NAME_PATTERN.test(value)) return "Enter a valid request header name.";
      if (SENSITIVE_HEADERS.has(value.toLocaleLowerCase())) {
        return "Sensitive request headers cannot be affinity sources.";
      }
    }
  }
  return null;
}
