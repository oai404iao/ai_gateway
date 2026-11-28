import type { ApiFormat, ApiOperation } from "@/api/types";

export type HeaderRuleKind = "set" | "remove" | "rename";
export type PatchRuleKind =
  | "add"
  | "replace"
  | "remove"
  | "array_append"
  | "array_prepend"
  | "array_insert"
  | "array_remove"
  | "merge";
export type PatchValueMode = "literal" | "current" | "template";
export type PatchConditionKind = "always" | "exists" | "missing" | "type" | "equals";
export type JsonValueType = "object" | "array" | "string" | "number" | "boolean" | "null";
export type EditorMode = "visual" | "json" | "reference";
export type HeaderScope = "request" | "response";
export type PatchScope = "request" | "sse";

export interface HeaderRule {
  id: string;
  kind: HeaderRuleKind;
  name: string;
  value: string;
}

export interface PatchRule {
  id: string;
  kind: PatchRuleKind;
  path: string;
  value: string;
  valueMode: PatchValueMode;
  index: string;
  condition: PatchConditionKind;
  conditionValue: string;
}

export interface SseRule {
  id: string;
  event: string;
  patches: PatchRule[];
}

export interface VisualDocument {
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

export interface TransformDocumentEditorProps {
  value: string;
  onChange: (value: string) => void;
  operation?: ApiOperation;
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

export const JSON_VALUE_TYPES: readonly JsonValueType[] = [
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

export function ruleId(): string {
  nextRuleId += 1;
  return `transform-rule-${nextRuleId}`;
}

export function emptyDocument(apiFormat: ApiFormat): VisualDocument {
  return {
    version: 1,
    apiFormat,
    requestHeaders: [],
    requestJson: [],
    responseHeaders: [],
    sse: [],
  };
}

export function newHeaderRule(): HeaderRule {
  return {
    id: ruleId(),
    kind: "set",
    name: "",
    value: "",
  };
}

export function newPatchRule(): PatchRule {
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

export function sseEvents(apiFormat: ApiFormat): readonly string[] {
  if (apiFormat === "open_ai_chat_completions") return CHAT_SSE_EVENTS;
  return apiFormat === "open_ai_responses" ? RESPONSES_SSE_EVENTS : [];
}

export function patchNeedsValue(kind: PatchRuleKind): boolean {
  return kind !== "remove" && kind !== "array_remove";
}

export function patchNeedsIndex(kind: PatchRuleKind): boolean {
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

export function patchRequiresExistingTarget(kind: PatchRuleKind): boolean {
  return kind !== "add";
}

export function patchRequiresV2(rule: PatchRule): boolean {
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

function parseSseRules(value: unknown, version: 1 | 2): SseRule[] | null {
  if (value === undefined) return [];
  if (!Array.isArray(value)) return null;

  const rules: SseRule[] = [];
  for (const item of value) {
    if (
      !isRecord(item) ||
      !hasOnlyKeys(item, ["event", "json"]) ||
      typeof item.event !== "string"
    ) {
      return null;
    }
    const patches = parsePatchRules(item.json, version);
    if (!patches) return null;
    rules.push({ id: ruleId(), event: item.event, patches });
  }
  return rules;
}

export function parseVisualDocument(
  value: string,
  fallbackApiFormat: ApiFormat,
  operation?: ApiOperation,
): ParseSuccess | ParseFailure {
  if (!value.trim() && !operation) return { document: emptyDocument(fallbackApiFormat) };

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
      decoded.api_format !== "open_ai_responses" &&
      decoded.api_format !== "open_ai_images")
  ) {
    return {
      error:
        "This JSON cannot be represented in the visual editor. Use JSON configuration to edit it.",
    };
  }

  const requestHeaders = parseHeaderRules(decoded.request_headers);
  const responseHeaders = parseHeaderRules(decoded.response_headers);
  const requestJson = parsePatchRules(decoded.request_json, decoded.version);
  const sse = parseSseRules(decoded.sse, decoded.version);
  const events = sseEvents(decoded.api_format);
  if (
    !requestHeaders || !responseHeaders || !requestJson || !sse ||
    (!operation && sse.some((rule) => !events.includes(rule.event)))
  ) {
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
    return [
      "host",
      "content-length",
      "content-encoding",
      "authorization",
      "proxy-authorization",
      "cookie",
      "accept-encoding",
    ].includes(normalized);
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
  if (apiFormat === "open_ai_images") return true;
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

export function supportsRequestJson(operation?: ApiOperation): boolean {
  return operation !== "images_edit" && operation !== "web_search";
}

export function supportsStreaming(apiFormat: ApiFormat, operation?: ApiOperation): boolean {
  return apiFormat !== "open_ai_images" && operation !== "web_search";
}

export function validateVisualDocument(document: VisualDocument, apiFormat: ApiFormat, operation?: ApiOperation): string | null {
  if (document.apiFormat !== apiFormat) return "The transform's API format must match the operation.";
  if (document.requestJson.length && !supportsRequestJson(operation)) {
    return "This operation does not support request-body rules.";
  }
  if (document.sse.length && !supportsStreaming(apiFormat, operation)) {
    return "This operation does not support streaming response rules.";
  }
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

export function hasRules(document: VisualDocument): boolean {
  return (
    document.requestHeaders.length > 0 ||
    document.requestJson.length > 0 ||
    document.responseHeaders.length > 0 ||
    document.sse.length > 0
  );
}

export function documentVersion(document: VisualDocument): 1 | 2 {
  return document.version === 2 ||
    [...document.requestJson, ...document.sse.flatMap((rule) => rule.patches)].some(patchRequiresV2)
    ? 2
    : 1;
}

export function serializeDocument(document: VisualDocument, apiFormat: ApiFormat): string {
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

