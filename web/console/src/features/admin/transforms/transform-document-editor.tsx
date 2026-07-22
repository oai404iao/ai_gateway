import { useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpen,
  Braces,
  CircleAlert,
  FileJson2,
  Plus,
  Trash2,
  WandSparkles,
} from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/app/i18n";
import type { ApiFormat } from "@/api/types";
import { apiFormatLabel, API_FORMATS } from "@/lib/permissions";

type HeaderRuleKind = "set" | "remove" | "rename";
type PatchRuleKind =
  | "add"
  | "replace"
  | "remove"
  | "array_append"
  | "array_prepend"
  | "array_insert"
  | "array_remove"
  | "merge";
type PatchValueMode = "literal" | "current" | "template";
type PatchConditionKind = "always" | "exists" | "missing" | "type" | "equals";
type JsonValueType = "object" | "array" | "string" | "number" | "boolean" | "null";
type EditorMode = "visual" | "json" | "reference";
type HeaderScope = "request" | "response";
type PatchScope = "request" | "sse";

interface HeaderRule {
  id: string;
  kind: HeaderRuleKind;
  name: string;
  value: string;
}

interface PatchRule {
  id: string;
  kind: PatchRuleKind;
  path: string;
  value: string;
  valueMode: PatchValueMode;
  index: string;
  condition: PatchConditionKind;
  conditionValue: string;
}

interface SseRule {
  id: string;
  event: string;
  patches: PatchRule[];
}

interface VisualDocument {
  version: 1 | 2;
  apiFormat: ApiFormat;
  requestHeaders: HeaderRule[];
  requestJson: PatchRule[];
  responseHeaders: HeaderRule[];
  sse: SseRule[];
}

interface ParseSuccess {
  document: VisualDocument;
}

interface ParseFailure {
  error: string;
}

interface TransformDocumentEditorProps {
  value: string;
  onChange: (value: string) => void;
  fixedApiFormat?: ApiFormat;
  defaultApiFormat?: ApiFormat;
  preserveWhenBlank?: boolean;
  onVisualValidationChange?: (message: string | null) => void;
}

const CHAT_SSE_EVENTS = ["chat.completion.chunk"] as const;
const RESPONSES_SSE_EVENTS = [
  "response.output_text.delta",
  "response.refusal.delta",
  "response.function_call_arguments.delta",
  "response.output_text.done",
  "response.completed",
] as const;

const JSON_VALUE_TYPES: readonly JsonValueType[] = [
  "object",
  "array",
  "string",
  "number",
  "boolean",
  "null",
];

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

let nextRuleId = 0;

function ruleId(): string {
  nextRuleId += 1;
  return `transform-rule-${nextRuleId}`;
}

function emptyDocument(apiFormat: ApiFormat): VisualDocument {
  return {
    version: 1,
    apiFormat,
    requestHeaders: [],
    requestJson: [],
    responseHeaders: [],
    sse: [],
  };
}

function newHeaderRule(): HeaderRule {
  return {
    id: ruleId(),
    kind: "set",
    name: "",
    value: "",
  };
}

function newPatchRule(): PatchRule {
  return {
    id: ruleId(),
    kind: "add",
    path: "",
    value: "null",
    valueMode: "literal",
    index: "",
    condition: "always",
    conditionValue: "",
  };
}

function sseEvents(apiFormat: ApiFormat): readonly string[] {
  return apiFormat === "open_ai_chat_completions" ? CHAT_SSE_EVENTS : RESPONSES_SSE_EVENTS;
}

function patchNeedsValue(kind: PatchRuleKind): boolean {
  return kind !== "remove" && kind !== "array_remove";
}

function patchNeedsIndex(kind: PatchRuleKind): boolean {
  return kind === "array_insert" || kind === "array_remove";
}

function patchIsArrayOperation(kind: PatchRuleKind): boolean {
  return (
    kind === "array_append" ||
    kind === "array_prepend" ||
    kind === "array_insert" ||
    kind === "array_remove"
  );
}

function patchRequiresExistingTarget(kind: PatchRuleKind): boolean {
  return kind !== "add";
}

function patchRequiresV2(rule: PatchRule): boolean {
  return (
    patchIsArrayOperation(rule.kind) ||
    rule.kind === "merge" ||
    rule.valueMode !== "literal" ||
    rule.condition !== "always"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasOnlyKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).every((key) => keys.includes(key));
}

function parseHeaderRules(value: unknown): HeaderRule[] | null {
  if (value === undefined) return [];
  if (!isRecord(value) || !hasOnlyKeys(value, ["set", "remove", "rename"])) return null;

  const rules: HeaderRule[] = [];
  if (value.set !== undefined) {
    if (!isRecord(value.set) || !Object.values(value.set).every((item) => typeof item === "string")) {
      return null;
    }
    for (const [name, headerValue] of Object.entries(value.set)) {
      rules.push({ id: ruleId(), kind: "set", name, value: headerValue as string });
    }
  }
  if (value.remove !== undefined) {
    if (!Array.isArray(value.remove) || !value.remove.every((item) => typeof item === "string")) {
      return null;
    }
    for (const name of value.remove) {
      rules.push({ id: ruleId(), kind: "remove", name, value: "" });
    }
  }
  if (value.rename !== undefined) {
    if (
      !isRecord(value.rename) ||
      !Object.values(value.rename).every((item) => typeof item === "string")
    ) {
      return null;
    }
    for (const [name, headerValue] of Object.entries(value.rename)) {
      rules.push({ id: ruleId(), kind: "rename", name, value: headerValue as string });
    }
  }
  return rules;
}

function parsePatchValue(value: unknown): Pick<PatchRule, "valueMode" | "value"> | null {
  if (isRecord(value) && Object.keys(value).length === 1 && value.$ref === "current") {
    return { valueMode: "current", value: "" };
  }
  if (isRecord(value) && Object.keys(value).length === 1 && typeof value.$template === "string") {
    return { valueMode: "template", value: value.$template };
  }
  const serialized = JSON.stringify(value, null, 2);
  return serialized === undefined ? null : { valueMode: "literal", value: serialized };
}

function parsePatchCondition(
  value: unknown,
): Pick<PatchRule, "condition" | "conditionValue"> | null {
  if (value === undefined) return { condition: "always", conditionValue: "" };
  if (!isRecord(value) || Object.keys(value).length !== 1) return null;
  if (typeof value.exists === "boolean") {
    return {
      condition: value.exists ? "exists" : "missing",
      conditionValue: "",
    };
  }
  if (typeof value.type === "string" && JSON_VALUE_TYPES.includes(value.type as JsonValueType)) {
    return { condition: "type", conditionValue: value.type };
  }
  if (Object.hasOwn(value, "equals")) {
    const serialized = JSON.stringify(value.equals, null, 2);
    return serialized === undefined ? null : { condition: "equals", conditionValue: serialized };
  }
  return null;
}

function parsePatchRules(value: unknown, version: 1 | 2): PatchRule[] | null {
  if (value === undefined) return [];
  if (!Array.isArray(value)) return null;

  const rules: PatchRule[] = [];
  for (const item of value) {
    const allowedKeys = version === 1 ? ["op", "path", "value"] : ["op", "path", "value", "index", "when"];
    if (!isRecord(item) || !hasOnlyKeys(item, allowedKeys) || typeof item.path !== "string") {
      return null;
    }
    const kind = item.op;
    if (
      kind !== "add" &&
      kind !== "replace" &&
      kind !== "remove" &&
      (version !== 2 ||
        (kind !== "array_append" &&
          kind !== "array_prepend" &&
          kind !== "array_insert" &&
          kind !== "array_remove" &&
          kind !== "merge"))
    ) {
      return null;
    }
    if (version === 1 && kind !== "add" && kind !== "replace" && kind !== "remove") return null;
    const needsValue = patchNeedsValue(kind);
    const needsIndex = patchNeedsIndex(kind);
    if (needsValue ? !Object.hasOwn(item, "value") : Object.hasOwn(item, "value")) return null;
    if (needsIndex) {
      if (!Number.isSafeInteger(item.index) || (item.index as number) < 0) return null;
    } else if (Object.hasOwn(item, "index")) {
      return null;
    }
    const parsedValue = needsValue
      ? parsePatchValue(item.value)
      : { valueMode: "literal" as const, value: "" };
    const parsedCondition = version === 2
      ? parsePatchCondition(item.when)
      : Object.hasOwn(item, "when")
        ? null
        : { condition: "always" as const, conditionValue: "" };
    if (!parsedValue || !parsedCondition) return null;
    rules.push({
      id: ruleId(),
      kind,
      path: item.path,
      value: parsedValue.value,
      valueMode: parsedValue.valueMode,
      index: needsIndex ? String(item.index) : "",
      condition: parsedCondition.condition,
      conditionValue: parsedCondition.conditionValue,
    });
  }
  return rules;
}

function parseSseRules(value: unknown, apiFormat: ApiFormat, version: 1 | 2): SseRule[] | null {
  if (value === undefined) return [];
  if (!Array.isArray(value)) return null;

  const allowedEvents = new Set(sseEvents(apiFormat));
  const rules: SseRule[] = [];
  for (const item of value) {
    if (
      !isRecord(item) ||
      !hasOnlyKeys(item, ["event", "json"]) ||
      typeof item.event !== "string" ||
      !allowedEvents.has(item.event)
    ) {
      return null;
    }
    const patches = parsePatchRules(item.json, version);
    if (!patches) return null;
    rules.push({ id: ruleId(), event: item.event, patches });
  }
  return rules;
}

function parseVisualDocument(
  value: string,
  fallbackApiFormat: ApiFormat,
): ParseSuccess | ParseFailure {
  if (!value.trim()) return { document: emptyDocument(fallbackApiFormat) };

  let decoded: unknown;
  try {
    decoded = JSON.parse(value);
  } catch {
    return { error: "Transform JSON is not valid. Open JSON configuration to repair it." };
  }
  if (!isRecord(decoded)) {
    return { error: "Transform document must be a JSON object." };
  }
  if (Object.keys(decoded).length === 0) {
    return { document: emptyDocument(fallbackApiFormat) };
  }
  if (
    !hasOnlyKeys(decoded, [
      "version",
      "api_format",
      "request_headers",
      "response_headers",
      "request_json",
      "sse",
    ]) ||
    (decoded.version !== 1 && decoded.version !== 2) ||
    (decoded.api_format !== "open_ai_chat_completions" &&
      decoded.api_format !== "open_ai_responses")
  ) {
    return {
      error:
        "This JSON cannot be represented in the visual editor. Use JSON configuration to edit it.",
    };
  }

  const requestHeaders = parseHeaderRules(decoded.request_headers);
  const responseHeaders = parseHeaderRules(decoded.response_headers);
  const requestJson = parsePatchRules(decoded.request_json, decoded.version);
  const sse = parseSseRules(decoded.sse, decoded.api_format, decoded.version);
  if (!requestHeaders || !responseHeaders || !requestJson || !sse) {
    return {
      error:
        "This JSON cannot be represented in the visual editor. Use JSON configuration to edit it.",
    };
  }
  return {
    document: {
      version: decoded.version,
      apiFormat: decoded.api_format,
      requestHeaders,
      requestJson,
      responseHeaders,
      sse,
    },
  };
}

function isHeaderName(value: string): boolean {
  return /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(value);
}

function isProtectedHeader(name: string, scope: HeaderScope): boolean {
  const normalized = name.toLowerCase();
  if (HOP_BY_HOP_HEADERS.has(normalized)) return true;
  if (scope === "request") {
    return ["host", "content-length", "authorization", "proxy-authorization", "cookie"].includes(
      normalized,
    );
  }
  return ["content-length", "content-type", "set-cookie", "content-encoding"].includes(normalized);
}

function parsePointer(path: string): string[] | null {
  if (!path || !path.startsWith("/")) return null;
  const tokens: string[] = [];
  for (const rawToken of path.slice(1).split("/")) {
    let token = "";
    for (let index = 0; index < rawToken.length; index += 1) {
      if (rawToken[index] !== "~") {
        token += rawToken[index];
        continue;
      }
      const escaped = rawToken[index + 1];
      if (escaped === "0") token += "~";
      else if (escaped === "1") token += "/";
      else return null;
      index += 1;
    }
    tokens.push(token);
  }
  return tokens;
}

function pointersConflict(left: string[], right: string[]): boolean {
  const [shorter, longer] = left.length <= right.length ? [left, right] : [right, left];
  return shorter.every((token, index) => token === longer[index]);
}

function protectedSsePointer(tokens: string[], apiFormat: ApiFormat): boolean {
  const first = tokens[0];
  if (apiFormat === "open_ai_chat_completions") {
    return (
      ["id", "object", "created", "model"].includes(first) ||
      (tokens.includes("choices") && tokens.length <= tokens.indexOf("choices") + 2) ||
      tokens.some((token) => ["id", "index", "item_id"].includes(token))
    );
  }
  const responseIndex = tokens.indexOf("response");
  const outputIndex = tokens.indexOf("output");
  return (
    ["type", "sequence_number"].includes(first) ||
    (responseIndex === 0 &&
      (tokens.length === 1 || ["id", "status"].includes(tokens[1] ?? ""))) ||
    (outputIndex >= 0 && tokens.length <= outputIndex + 2) ||
    tokens.some((token) =>
      [
        "id",
        "item_id",
        "call_id",
        "response_id",
        "index",
        "output_index",
        "content_index",
        "type",
        "sequence_number",
      ].includes(token),
    )
  );
}

function validateHeaderRules(rules: HeaderRule[], scope: HeaderScope): string | null {
  const touched = new Set<string>();
  for (const rule of rules) {
    if (!rule.name || !isHeaderName(rule.name) || isProtectedHeader(rule.name, scope)) {
      return "Header names must be valid and cannot be protected headers.";
    }
    const name = rule.name.toLowerCase();
    if (touched.has(name)) {
      return "A header can only appear in one operation.";
    }
    touched.add(name);
    if (rule.kind === "rename") {
      if (!rule.value || !isHeaderName(rule.value) || isProtectedHeader(rule.value, scope)) {
        return "Header names must be valid and cannot be protected headers.";
      }
      const destination = rule.value.toLowerCase();
      if (touched.has(destination)) {
        return "A header can only appear in one operation.";
      }
      touched.add(destination);
    }
  }
  return null;
}

function validatePatchRules(
  rules: PatchRule[],
  scope: PatchScope,
  apiFormat: ApiFormat,
): string | null {
  const paths: Array<{ tokens: string[]; kind: PatchRuleKind }> = [];
  for (const rule of rules) {
    const tokens = parsePointer(rule.path);
    if (!tokens) return "Each JSON Patch needs a non-root RFC 6901 JSON Pointer.";
    if (scope === "request" && tokens.some((token) => token === "model" || token === "stream")) {
      return "Request-body rules cannot change model or stream.";
    }
    if (scope === "sse" && protectedSsePointer(tokens, apiFormat)) {
      return "Streaming response rules cannot change immutable event envelope fields.";
    }
    if (patchNeedsIndex(rule.kind)) {
      const index = Number(rule.index);
      if (!Number.isSafeInteger(index) || index < 0) {
        return "Array insert and remove need a non-negative integer index.";
      }
    }
    if (patchNeedsValue(rule.kind) && rule.valueMode === "literal") {
      try {
        JSON.parse(rule.value);
      } catch {
        return "Every value rule needs a valid JSON value.";
      }
    }
    if (patchNeedsValue(rule.kind) && rule.valueMode === "template" && !rule.value) {
      return "A string template cannot be blank.";
    }
    if (rule.condition === "type" && !JSON_VALUE_TYPES.includes(rule.conditionValue as JsonValueType)) {
      return "Choose a JSON type for the condition.";
    }
    if (rule.condition === "missing" && patchRequiresExistingTarget(rule.kind)) {
      return "This operation requires an existing target and cannot run when it is missing.";
    }
    if (rule.condition === "equals") {
      try {
        JSON.parse(rule.conditionValue);
      } catch {
        return "The equals condition needs a valid JSON value.";
      }
    }
    if (paths.some((other) => pointersConflict(other.tokens, tokens))) {
      const conflicts = paths.filter((other) => pointersConflict(other.tokens, tokens));
      const onlySamePathArrayOperations = conflicts.every(
        (other) =>
          other.tokens.length === tokens.length &&
          other.tokens.every((token, index) => token === tokens[index]) &&
          patchIsArrayOperation(other.kind) &&
          patchIsArrayOperation(rule.kind),
      );
      if (!onlySamePathArrayOperations) {
        return "JSON Patch paths cannot overlap in the same rule list.";
      }
    }
    paths.push({ tokens, kind: rule.kind });
  }
  return null;
}

function validateVisualDocument(document: VisualDocument, apiFormat: ApiFormat): string | null {
  const requestHeadersError = validateHeaderRules(document.requestHeaders, "request");
  if (requestHeadersError) return requestHeadersError;
  const requestPatchError = validatePatchRules(document.requestJson, "request", apiFormat);
  if (requestPatchError) return requestPatchError;
  const responseHeadersError = validateHeaderRules(document.responseHeaders, "response");
  if (responseHeadersError) return responseHeadersError;

  const seenEvents = new Set<string>();
  const allowedEvents = new Set(sseEvents(apiFormat));
  for (const rule of document.sse) {
    if (!allowedEvents.has(rule.event)) {
      return "Choose a response event supported by the selected API format.";
    }
    if (seenEvents.has(rule.event)) {
      return "Each streaming response event can have only one rule.";
    }
    seenEvents.add(rule.event);
    if (rule.patches.length === 0) {
      return "Each streaming response event rule needs at least one JSON Patch.";
    }
    const patchError = validatePatchRules(rule.patches, "sse", apiFormat);
    if (patchError) return patchError;
  }
  return null;
}

function headerDocument(rules: HeaderRule[]): Record<string, unknown> | undefined {
  if (rules.length === 0) return undefined;

  const set: Record<string, string> = {};
  const remove: string[] = [];
  const rename: Record<string, string> = {};
  for (const rule of rules) {
    if (rule.kind === "set") set[rule.name] = rule.value;
    else if (rule.kind === "remove") remove.push(rule.name);
    else rename[rule.name] = rule.value;
  }
  const result: Record<string, unknown> = {};
  if (Object.keys(set).length > 0) result.set = set;
  if (remove.length > 0) result.remove = remove;
  if (Object.keys(rename).length > 0) result.rename = rename;
  return result;
}

function patchValueDocument(rule: PatchRule): unknown {
  if (rule.valueMode === "current") return { $ref: "current" };
  if (rule.valueMode === "template") return { $template: rule.value };
  return JSON.parse(rule.value);
}

function patchConditionDocument(rule: PatchRule): Record<string, unknown> | undefined {
  if (rule.condition === "always") return undefined;
  if (rule.condition === "exists") return { exists: true };
  if (rule.condition === "missing") return { exists: false };
  if (rule.condition === "type") return { type: rule.conditionValue };
  return { equals: JSON.parse(rule.conditionValue) };
}

function patchDocument(rules: PatchRule[], version: 1 | 2): Array<Record<string, unknown>> {
  return rules.map((rule) => {
    const result: Record<string, unknown> = { op: rule.kind, path: rule.path };
    if (patchNeedsValue(rule.kind)) result.value = patchValueDocument(rule);
    if (version === 2 && patchNeedsIndex(rule.kind)) result.index = Number(rule.index);
    const when = version === 2 ? patchConditionDocument(rule) : undefined;
    if (when) result.when = when;
    return result;
  });
}

function hasRules(document: VisualDocument): boolean {
  return (
    document.requestHeaders.length > 0 ||
    document.requestJson.length > 0 ||
    document.responseHeaders.length > 0 ||
    document.sse.length > 0
  );
}

function documentVersion(document: VisualDocument): 1 | 2 {
  return document.version === 2 ||
    [...document.requestJson, ...document.sse.flatMap((rule) => rule.patches)].some(patchRequiresV2)
    ? 2
    : 1;
}

function serializeDocument(document: VisualDocument, apiFormat: ApiFormat): string {
  if (!hasRules(document)) return "{}";
  const version = documentVersion(document);

  const result: Record<string, unknown> = {
    version,
    api_format: apiFormat,
  };
  const requestHeaders = headerDocument(document.requestHeaders);
  const responseHeaders = headerDocument(document.responseHeaders);
  if (requestHeaders) result.request_headers = requestHeaders;
  if (document.requestJson.length > 0) result.request_json = patchDocument(document.requestJson, version);
  if (responseHeaders) result.response_headers = responseHeaders;
  if (document.sse.length > 0) {
    result.sse = document.sse.map((rule) => ({
      event: rule.event,
      json: patchDocument(rule.patches, version),
    }));
  }
  return JSON.stringify(result, null, 2);
}

function sectionEmpty(title: string, description: string) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <Braces />
        </EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        <EmptyDescription>{description}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

function HeaderRulesEditor({
  idPrefix,
  rules,
  scope,
  onChange,
}: {
  idPrefix: string;
  rules: HeaderRule[];
  scope: HeaderScope;
  onChange: (rules: HeaderRule[]) => void;
}) {
  const { t } = useI18n();
  const updateRule = (index: number, partial: Partial<HeaderRule>) => {
    onChange(rules.map((rule, ruleIndex) => (ruleIndex === index ? { ...rule, ...partial } : rule)));
  };

  if (rules.length === 0) {
    return sectionEmpty(
      scope === "request" ? t("No request-header rules") : t("No response-header rules"),
      scope === "request"
        ? t("Start with a header that identifies or adapts the upstream request.")
        : t("Add a safe response header for the client."),
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {rules.map((rule, index) => (
        <div key={rule.id} className="flex flex-col gap-4">
          {index > 0 ? <Separator /> : null}
          <FieldGroup>
            <Field orientation="responsive">
              <FieldLabel>{t("Operation")}</FieldLabel>
              <Select
                value={rule.kind}
                onValueChange={(value) => updateRule(index, { kind: value as HeaderRuleKind })}
              >
                <SelectTrigger aria-label={t("Header operation")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="set">{t("Set header")}</SelectItem>
                    <SelectItem value="remove">{t("Remove header")}</SelectItem>
                    <SelectItem value="rename">{t("Rename header")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field orientation="responsive">
              <FieldLabel htmlFor={`${idPrefix}-${rule.id}-name`}>{t("Header name")}</FieldLabel>
              <Input
                id={`${idPrefix}-${rule.id}-name`}
                value={rule.name}
                onChange={(event) => updateRule(index, { name: event.target.value })}
                placeholder="x-gateway-trace"
              />
            </Field>
            {rule.kind !== "remove" ? (
              <Field orientation="responsive">
                <FieldLabel htmlFor={`${idPrefix}-${rule.id}-value`}>
                  {rule.kind === "rename" ? t("New header name") : t("Header value")}
                </FieldLabel>
                <Input
                  id={`${idPrefix}-${rule.id}-value`}
                  value={rule.value}
                  onChange={(event) => updateRule(index, { value: event.target.value })}
                  placeholder={rule.kind === "rename" ? "x-client-trace" : "console"}
                />
              </Field>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="self-start"
              onClick={() => onChange(rules.filter((_, ruleIndex) => ruleIndex !== index))}
            >
              <Trash2 data-icon="inline-start" />
              {t("Delete rule")}
            </Button>
          </FieldGroup>
        </div>
      ))}
    </div>
  );
}

function PatchRulesEditor({
  idPrefix,
  rules,
  scope,
  onChange,
}: {
  idPrefix: string;
  rules: PatchRule[];
  scope: PatchScope;
  onChange: (rules: PatchRule[]) => void;
}) {
  const { t } = useI18n();
  const updateRule = (index: number, partial: Partial<PatchRule>) => {
    onChange(rules.map((rule, ruleIndex) => (ruleIndex === index ? { ...rule, ...partial } : rule)));
  };

  if (rules.length === 0) {
    return sectionEmpty(
      scope === "request" ? t("No request-body rules") : t("No response-body rules"),
      scope === "request"
        ? t("Add a JSON Patch to adjust an upstream request.")
        : t("Add a JSON Patch for the selected streaming event."),
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {rules.map((rule, index) => (
        <div key={rule.id} className="flex flex-col gap-4">
          {index > 0 ? <Separator /> : null}
          <FieldGroup>
            <Field orientation="responsive">
              <FieldLabel>{t("Operation")}</FieldLabel>
              <Select
                value={rule.kind}
                onValueChange={(value) => {
                  const kind = value as PatchRuleKind;
                  updateRule(index, {
                    kind,
                    index: patchNeedsIndex(kind) ? rule.index : "",
                    value: patchNeedsValue(kind) ? rule.value : "",
                    valueMode: patchNeedsValue(kind) ? rule.valueMode : "literal",
                  });
                }}
              >
                <SelectTrigger aria-label={t("JSON Patch operation")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="add">{t("Add value")}</SelectItem>
                    <SelectItem value="replace">{t("Replace value")}</SelectItem>
                    <SelectItem value="remove">{t("Remove value")}</SelectItem>
                    <SelectItem value="array_append">{t("Append to array")}</SelectItem>
                    <SelectItem value="array_prepend">{t("Prepend to array")}</SelectItem>
                    <SelectItem value="array_insert">{t("Insert into array")}</SelectItem>
                    <SelectItem value="array_remove">{t("Remove array item")}</SelectItem>
                    <SelectItem value="merge">{t("Merge object")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field orientation="responsive">
              <FieldLabel htmlFor={`${idPrefix}-${rule.id}-path`}>{t("JSON pointer")}</FieldLabel>
              <Input
                id={`${idPrefix}-${rule.id}-path`}
                value={rule.path}
                onChange={(event) => updateRule(index, { path: event.target.value })}
                placeholder="/temperature"
                className="font-mono text-xs"
              />
            </Field>
            {patchNeedsIndex(rule.kind) ? (
              <Field orientation="responsive">
                <FieldLabel htmlFor={`${idPrefix}-${rule.id}-index`}>{t("Array index")}</FieldLabel>
                <Input
                  id={`${idPrefix}-${rule.id}-index`}
                  type="number"
                  min={0}
                  step={1}
                  value={rule.index}
                  onChange={(event) => updateRule(index, { index: event.target.value })}
                />
              </Field>
            ) : null}
            {patchNeedsValue(rule.kind) ? (
              <>
                <Field orientation="responsive">
                  <FieldLabel>{t("Value source")}</FieldLabel>
                  <Select
                    value={rule.valueMode}
                    onValueChange={(value) => {
                      const valueMode = value as PatchValueMode;
                      updateRule(index, {
                        valueMode,
                        value:
                          valueMode === "literal"
                            ? rule.valueMode === "literal"
                              ? rule.value
                              : "null"
                            : valueMode === "template"
                              ? rule.valueMode === "template"
                                ? rule.value
                                : "{{value}}"
                              : "",
                      });
                    }}
                  >
                    <SelectTrigger aria-label={t("Value source")}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="literal">{t("Literal JSON")}</SelectItem>
                        <SelectItem value="current">{t("Current path value")}</SelectItem>
                        <SelectItem value="template">{t("String template")}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                {rule.valueMode === "literal" ? (
                  <Field>
                    <FieldLabel htmlFor={`${idPrefix}-${rule.id}-value`}>
                      {t("Value (JSON)")}
                    </FieldLabel>
                    <Textarea
                      id={`${idPrefix}-${rule.id}-value`}
                      rows={2}
                      value={rule.value}
                      onChange={(event) => updateRule(index, { value: event.target.value })}
                      className="font-mono text-xs"
                    />
                    <FieldDescription>
                      {t("Use valid JSON, including quotes around strings.")}
                    </FieldDescription>
                  </Field>
                ) : null}
                {rule.valueMode === "current" ? (
                  <Field>
                    <FieldDescription>
                      {t(
                        "Copies the value at this rule's target path before the operation runs.",
                      )}
                    </FieldDescription>
                  </Field>
                ) : null}
                {rule.valueMode === "template" ? (
                  <Field>
                    <FieldLabel htmlFor={`${idPrefix}-${rule.id}-template`}>
                      {t("String template")}
                    </FieldLabel>
                    <Textarea
                      id={`${idPrefix}-${rule.id}-template`}
                      rows={2}
                      value={rule.value}
                      onChange={(event) => updateRule(index, { value: event.target.value })}
                      placeholder="gateway-{{value}}"
                      className="font-mono text-xs"
                    />
                    <FieldDescription>
                      {t("Use {{value}} to interpolate the current value as text.")}
                    </FieldDescription>
                  </Field>
                ) : null}
              </>
            ) : null}
            <Field orientation="responsive">
              <FieldLabel>{t("Run when")}</FieldLabel>
              <Select
                value={rule.condition}
                onValueChange={(value) => {
                  const condition = value as PatchConditionKind;
                  updateRule(index, {
                    condition,
                    conditionValue:
                      condition === rule.condition
                        ? rule.conditionValue
                        : condition === "type"
                          ? "string"
                          : "",
                  });
                }}
              >
                <SelectTrigger aria-label={t("Run when")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="always">{t("Always")}</SelectItem>
                    <SelectItem value="exists">{t("Path exists")}</SelectItem>
                    <SelectItem
                      value="missing"
                      disabled={patchRequiresExistingTarget(rule.kind)}
                    >
                      {t("Path is missing")}
                    </SelectItem>
                    <SelectItem value="type">{t("Path has JSON type")}</SelectItem>
                    <SelectItem value="equals">{t("Path equals JSON")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            {rule.condition === "type" ? (
              <Field orientation="responsive">
                <FieldLabel>{t("JSON value type")}</FieldLabel>
                <Select
                  value={rule.conditionValue || "string"}
                  onValueChange={(conditionValue) => updateRule(index, { conditionValue })}
                >
                  <SelectTrigger aria-label={t("JSON value type")}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {JSON_VALUE_TYPES.map((type) => (
                        <SelectItem key={type} value={type}>
                          {type}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            {rule.condition === "equals" ? (
              <Field>
                <FieldLabel htmlFor={`${idPrefix}-${rule.id}-equals`}>
                  {t("Equals value (JSON)")}
                </FieldLabel>
                <Textarea
                  id={`${idPrefix}-${rule.id}-equals`}
                  rows={2}
                  value={rule.conditionValue}
                  onChange={(event) => updateRule(index, { conditionValue: event.target.value })}
                  className="font-mono text-xs"
                />
              </Field>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="self-start"
              onClick={() => onChange(rules.filter((_, ruleIndex) => ruleIndex !== index))}
            >
              <Trash2 data-icon="inline-start" />
              {t("Delete rule")}
            </Button>
          </FieldGroup>
        </div>
      ))}
    </div>
  );
}

function ReferenceExample({
  title,
  description,
  document,
  onApply,
}: {
  title: string;
  description: string;
  document: Record<string, unknown>;
  onApply: () => void;
}) {
  const { t } = useI18n();
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <pre className="overflow-x-auto rounded-lg border p-3 font-mono text-xs">
          {JSON.stringify(document, null, 2)}
        </pre>
        <Button type="button" variant="outline" size="sm" className="self-start" onClick={onApply}>
          <WandSparkles data-icon="inline-start" />
          {t("Apply example")}
        </Button>
      </CardContent>
    </Card>
  );
}

export function TransformDocumentEditor({
  value,
  onChange,
  fixedApiFormat,
  defaultApiFormat,
  preserveWhenBlank = false,
  onVisualValidationChange,
}: TransformDocumentEditorProps) {
  const { t } = useI18n();
  const fallbackApiFormat = fixedApiFormat ?? defaultApiFormat ?? API_FORMATS[0];
  const initial = parseVisualDocument(value, fallbackApiFormat);
  const [mode, setMode] = useState<EditorMode>("visual");
  const [document, setDocument] = useState<VisualDocument>(() =>
    "document" in initial ? initial.document : emptyDocument(fallbackApiFormat),
  );
  const [visualError, setVisualError] = useState<string | null>(
    "error" in initial ? initial.error : null,
  );
  const emittedValue = useRef<string | null>(null);
  const previousFallbackApiFormat = useRef<ApiFormat>(fallbackApiFormat);

  useEffect(() => {
    const fallbackChanged = previousFallbackApiFormat.current !== fallbackApiFormat;
    previousFallbackApiFormat.current = fallbackApiFormat;
    if (value === emittedValue.current && !fallbackChanged) return;

    const parsed = parseVisualDocument(value, fallbackApiFormat);
    if ("error" in parsed) {
      setVisualError(parsed.error);
      onVisualValidationChange?.(null);
      return;
    }
    setDocument({
      ...parsed.document,
      apiFormat: fixedApiFormat ?? parsed.document.apiFormat,
    });
    setVisualError(null);
    onVisualValidationChange?.(null);
  }, [fallbackApiFormat, fixedApiFormat, onVisualValidationChange, value]);

  const apiFormat = fixedApiFormat ?? document.apiFormat;
  const activeDocumentVersion = documentVersion(document);
  const availableSseEvents = useMemo(
    () => sseEvents(apiFormat).filter((event) => !document.sse.some((rule) => rule.event === event)),
    [apiFormat, document.sse],
  );

  const commitVisualDocument = (next: VisualDocument, writeNoop = true) => {
    setDocument(next);
    const nextApiFormat = fixedApiFormat ?? next.apiFormat;
    const error = validateVisualDocument(next, nextApiFormat);
    setVisualError(error);
    onVisualValidationChange?.(error);
    if (error || (!writeNoop && !hasRules(next))) return;

    const nextValue = serializeDocument(next, nextApiFormat);
    emittedValue.current = nextValue;
    onChange(nextValue);
  };

  const applyExample = (example: Record<string, unknown>) => {
    const nextValue = JSON.stringify(example, null, 2);
    const parsed = parseVisualDocument(nextValue, fallbackApiFormat);
    if ("error" in parsed) return;
    const nextDocument = {
      ...parsed.document,
      apiFormat: fixedApiFormat ?? parsed.document.apiFormat,
    };
    emittedValue.current = nextValue;
    setDocument(nextDocument);
    setVisualError(null);
    onVisualValidationChange?.(null);
    onChange(nextValue);
    setMode("visual");
  };

  const requestHeaderExample = {
    version: 1,
    api_format: apiFormat,
    request_headers: {
      set: {
        "x-gateway-source": "console",
      },
    },
  };
  const requestBodyExample = {
    version: 1,
    api_format: apiFormat,
    request_json: [
      {
        op: "replace",
        path: "/temperature",
        value: 0.2,
      },
    ],
  };
  const arrayRewriteExample = {
    version: 2,
    api_format: apiFormat,
    request_json: [
      {
        op: "array_prepend",
        path: "/messages",
        value: {
          role: "system",
          content: "Follow the gateway policy.",
        },
        when: {
          type: "array",
        },
      },
    ],
  };
  const currentValueExample = {
    version: 2,
    api_format: apiFormat,
    request_json: [
      {
        op: "replace",
        path: "/metadata",
        value: {
          original: {
            $ref: "current",
          },
          gateway: "console",
        },
        when: {
          type: "object",
        },
      },
    ],
  };
  const conditionalMergeExample = {
    version: 2,
    api_format: apiFormat,
    request_json: [
      {
        op: "merge",
        path: "/metadata",
        value: {
          gateway: "console",
        },
        when: {
          type: "object",
        },
      },
    ],
  };
  const responseHeaderExample = {
    version: 1,
    api_format: apiFormat,
    response_headers: {
      set: {
        "x-gateway-transform": "enabled",
      },
    },
  };
  const responseBodyExample = {
    version: 1,
    api_format: apiFormat,
    sse: [
      apiFormat === "open_ai_chat_completions"
        ? {
            event: "chat.completion.chunk",
            json: [
              {
                op: "add",
                path: "/choices/0/delta/gateway_trace",
                value: "proxied",
              },
            ],
          }
        : {
            event: "response.output_text.delta",
            json: [
              {
                op: "add",
                path: "/gateway_trace",
                value: "proxied",
              },
            ],
          },
    ],
  };

  return (
    <Tabs value={mode} onValueChange={(next) => setMode(next as EditorMode)}>
      <TabsList aria-label={t("Transform editor views")}>
        <TabsTrigger value="visual">
          <Braces data-icon="inline-start" />
          {t("Visual editor")}
        </TabsTrigger>
        <TabsTrigger value="json">
          <FileJson2 data-icon="inline-start" />
          {t("JSON configuration")}
        </TabsTrigger>
        <TabsTrigger value="reference">
          <BookOpen data-icon="inline-start" />
          {t("Reference")}
        </TabsTrigger>
      </TabsList>

      <TabsContent value="visual" className="flex flex-col gap-4">
        {preserveWhenBlank && !value.trim() ? (
          <Alert>
            <CircleAlert />
            <AlertTitle>{t("Stored transform is redacted")}</AlertTitle>
            <AlertDescription>
              {t(
                "The existing document is not returned by the API. Adding or applying a visual rule replaces it; leave this editor untouched to preserve it.",
              )}
            </AlertDescription>
          </Alert>
        ) : null}
        {visualError ? (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertTitle>{t("Review the transform rules")}</AlertTitle>
            <AlertDescription>{t(visualError)}</AlertDescription>
          </Alert>
        ) : null}

        <Card>
          <CardHeader>
            <CardTitle>{t("Transform scope")}</CardTitle>
            <CardDescription>
              {t(
                "Templates run first, then channel overrides. Upstream authentication is applied last and cannot be changed by transform rules.",
              )}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <FieldGroup>
              <Field>
                <FieldLabel>{t("Rule format")}</FieldLabel>
                {fixedApiFormat ? (
                  <Badge variant="secondary">{apiFormatLabel(fixedApiFormat)}</Badge>
                ) : (
                  <Select
                    value={document.apiFormat}
                    onValueChange={(next) => {
                      const nextApiFormat = next as ApiFormat;
                      if (
                        document.sse.length > 0 &&
                        nextApiFormat !== document.apiFormat
                      ) {
                        const message =
                          "Remove streaming response rules before changing the API format.";
                        setVisualError(message);
                        onVisualValidationChange?.(message);
                        return;
                      }
                      commitVisualDocument({ ...document, apiFormat: nextApiFormat }, false);
                    }}
                  >
                    <SelectTrigger aria-label={t("Rule format")}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {API_FORMATS.map((format) => (
                          <SelectItem key={format} value={format}>
                            {apiFormatLabel(format)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                )}
                <FieldDescription>
                  {fixedApiFormat
                    ? t("The channel group fixes this transform's API format.")
                    : t("Choose the API format before adding streaming response rules.")}
                </FieldDescription>
              </Field>
              <Field>
                <FieldLabel>{t("Transform DSL version")}</FieldLabel>
                <Badge variant="secondary">v{activeDocumentVersion}</Badge>
                <FieldDescription>
                  {activeDocumentVersion === 1
                    ? t("Version 1 uses standard JSON Patch operations.")
                    : t(
                        "Version 2 adds bounded array edits, shallow object merge, target-value references, and conditions.",
                      )}
                </FieldDescription>
              </Field>
            </FieldGroup>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-start justify-between gap-4">
            <div className="flex flex-col gap-1.5">
              <CardTitle>{t("Request headers")}</CardTitle>
              <CardDescription>
                {t("Set, remove, or rename safe headers before the request reaches the upstream.")}
              </CardDescription>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                commitVisualDocument({
                  ...document,
                  requestHeaders: [...document.requestHeaders, newHeaderRule()],
                })
              }
            >
              <Plus data-icon="inline-start" />
              {t("Add request header rule")}
            </Button>
          </CardHeader>
          <CardContent>
            <HeaderRulesEditor
              idPrefix="request-header"
              rules={document.requestHeaders}
              scope="request"
              onChange={(requestHeaders) => commitVisualDocument({ ...document, requestHeaders })}
            />
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-start justify-between gap-4">
            <div className="flex flex-col gap-1.5">
              <CardTitle>{t("Request body (JSON Patch)")}</CardTitle>
              <CardDescription>
                {t(
                  "Apply JSON Patch or version 2 array and object operations to the upstream JSON request. Model and stream remain protected.",
                )}
              </CardDescription>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                commitVisualDocument({
                  ...document,
                  requestJson: [...document.requestJson, newPatchRule()],
                })
              }
            >
              <Plus data-icon="inline-start" />
              {t("Add request body rule")}
            </Button>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <PatchRulesEditor
              idPrefix="request-json"
              rules={document.requestJson}
              scope="request"
              onChange={(requestJson) => commitVisualDocument({ ...document, requestJson })}
            />
            <Alert>
              <CircleAlert />
              <AlertTitle>{t("JSON Patch note")}</AlertTitle>
              <AlertDescription>
                {t(
                  "Replace and remove require a path that already exists. Add also requires its parent path to exist.",
                )}
              </AlertDescription>
            </Alert>
            <Alert>
              <CircleAlert />
              <AlertTitle>{t("Version 2 safety boundary")}</AlertTitle>
              <AlertDescription>
                {t(
                  "Array operations target an existing array, merge is shallow and targets an existing object, and {{value}} can only read this rule's target before it runs.",
                )}
              </AlertDescription>
            </Alert>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-start justify-between gap-4">
            <div className="flex flex-col gap-1.5">
              <CardTitle>{t("Response headers")}</CardTitle>
              <CardDescription>
                {t("Set, remove, or rename safe headers before the response is sent to the client.")}
              </CardDescription>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                commitVisualDocument({
                  ...document,
                  responseHeaders: [...document.responseHeaders, newHeaderRule()],
                })
              }
            >
              <Plus data-icon="inline-start" />
              {t("Add response header rule")}
            </Button>
          </CardHeader>
          <CardContent>
            <HeaderRulesEditor
              idPrefix="response-header"
              rules={document.responseHeaders}
              scope="response"
              onChange={(responseHeaders) => commitVisualDocument({ ...document, responseHeaders })}
            />
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-start justify-between gap-4">
            <div className="flex flex-col gap-1.5">
              <CardTitle>{t("Response body (streaming SSE)")}</CardTitle>
              <CardDescription>
                {t(
                  "Patch supported JSON Server-Sent Events. Non-streaming JSON response bodies always pass through unchanged.",
                )}
              </CardDescription>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={availableSseEvents.length === 0}
              onClick={() => {
                const event = availableSseEvents[0];
                if (!event) return;
                commitVisualDocument({
                  ...document,
                  sse: [...document.sse, { id: ruleId(), event, patches: [] }],
                });
              }}
            >
              <Plus data-icon="inline-start" />
              {t("Add streaming response rule")}
            </Button>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            {document.sse.length === 0
              ? sectionEmpty(
                  t("No response-body rules"),
                  t("Add a supported streaming event and JSON Patch operations."),
                )
              : document.sse.map((rule, index) => (
                  <Card key={rule.id}>
                    <CardHeader className="flex flex-row items-start justify-between gap-4">
                      <div className="flex flex-col gap-1.5">
                        <CardTitle>{t("Streaming event rule")} {index + 1}</CardTitle>
                        <CardDescription>
                          {t("Each event can have one rule with one or more JSON Patch operations.")}
                        </CardDescription>
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() =>
                          commitVisualDocument({
                            ...document,
                            sse: document.sse.filter((_, ruleIndex) => ruleIndex !== index),
                          })
                        }
                      >
                        <Trash2 data-icon="inline-start" />
                        {t("Delete rule")}
                      </Button>
                    </CardHeader>
                    <CardContent className="flex flex-col gap-4">
                      <FieldGroup>
                        <Field>
                          <FieldLabel>{t("Event")}</FieldLabel>
                          <Select
                            value={rule.event}
                            onValueChange={(event) =>
                              commitVisualDocument({
                                ...document,
                                sse: document.sse.map((item, ruleIndex) =>
                                  ruleIndex === index ? { ...item, event } : item,
                                ),
                              })
                            }
                          >
                            <SelectTrigger aria-label={t("Streaming response event")}>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectGroup>
                                {sseEvents(apiFormat).map((event) => (
                                  <SelectItem
                                    key={event}
                                    value={event}
                                    disabled={document.sse.some(
                                      (item, ruleIndex) =>
                                        ruleIndex !== index && item.event === event,
                                    )}
                                  >
                                    {event}
                                  </SelectItem>
                                ))}
                              </SelectGroup>
                            </SelectContent>
                          </Select>
                        </Field>
                      </FieldGroup>
                      <PatchRulesEditor
                        idPrefix={`sse-${rule.id}`}
                        rules={rule.patches}
                        scope="sse"
                        onChange={(patches) =>
                          commitVisualDocument({
                            ...document,
                            sse: document.sse.map((item, ruleIndex) =>
                              ruleIndex === index ? { ...item, patches } : item,
                            ),
                          })
                        }
                      />
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="self-start"
                        onClick={() =>
                          commitVisualDocument({
                            ...document,
                            sse: document.sse.map((item, ruleIndex) =>
                              ruleIndex === index
                                ? { ...item, patches: [...item.patches, newPatchRule()] }
                                : item,
                            ),
                          })
                        }
                      >
                        <Plus data-icon="inline-start" />
                        {t("Add event patch")}
                      </Button>
                    </CardContent>
                  </Card>
                ))}
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="json">
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="transform-document-json">{t("Transform document JSON")}</FieldLabel>
            <FieldDescription>
              {t(
                "Use this mode for direct editing. Visual editor supports transform schema versions 1 and 2 and keeps its generated JSON synchronized.",
              )}
            </FieldDescription>
            <Textarea
              id="transform-document-json"
              rows={24}
              value={value}
              onChange={(event) => {
                emittedValue.current = null;
                setVisualError(null);
                onVisualValidationChange?.(null);
                onChange(event.target.value);
              }}
              className="font-mono text-xs"
              spellCheck={false}
            />
          </Field>
        </FieldGroup>
      </TabsContent>

      <TabsContent value="reference" className="flex flex-col gap-4">
        <Alert>
          <CircleAlert />
          <AlertTitle>{t("Transform rule limits")}</AlertTitle>
          <AlertDescription>
            {t(
              "Request rules cannot change authorization, cookies, host, content length, model, or stream. Response rules cannot change content type, content length, encoding, or cookies. Hop-by-hop headers are always protected.",
            )}
          </AlertDescription>
        </Alert>
        <Alert>
          <CircleAlert />
          <AlertTitle>{t("Response-body scope")}</AlertTitle>
          <AlertDescription>
            {t(
              "Response-body patches run only for the listed streaming SSE events. They do not buffer or rewrite ordinary non-streaming JSON responses.",
            )}
          </AlertDescription>
        </Alert>
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <ReferenceExample
            title={t("Request header example")}
            description={t("Add an upstream marker without touching protected credentials.")}
            document={requestHeaderExample}
            onApply={() => applyExample(requestHeaderExample)}
          />
          <ReferenceExample
            title={t("Request body example")}
            description={t("Replace a value that the client request already contains.")}
            document={requestBodyExample}
            onApply={() => applyExample(requestBodyExample)}
          />
          <ReferenceExample
            title={t("Array rewrite example")}
            description={t("Prepend a system message only when the target path is an array.")}
            document={arrayRewriteExample}
            onApply={() => applyExample(arrayRewriteExample)}
          />
          <ReferenceExample
            title={t("Current-value reference example")}
            description={t("Keep the original target value inside a replacement object.")}
            document={currentValueExample}
            onApply={() => applyExample(currentValueExample)}
          />
          <ReferenceExample
            title={t("Conditional merge example")}
            description={t("Shallow-merge safe metadata only when the target is an object.")}
            document={conditionalMergeExample}
            onApply={() => applyExample(conditionalMergeExample)}
          />
          <ReferenceExample
            title={t("Response header example")}
            description={t("Expose a safe marker to clients in the upstream response.")}
            document={responseHeaderExample}
            onApply={() => applyExample(responseHeaderExample)}
          />
          <ReferenceExample
            title={t("Streaming response example")}
            description={t("Add a non-envelope field to a supported streaming event.")}
            document={responseBodyExample}
            onApply={() => applyExample(responseBodyExample)}
          />
        </div>
      </TabsContent>
    </Tabs>
  );
}
