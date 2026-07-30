export type CodexImportSource = "ai_gateway" | "cpa" | "sub2api" | "generic";

export interface CodexImportDocument {
  name: string;
  content: string;
}

export interface ImportedProxyDraft {
  id: string;
  source: CodexImportSource;
  source_name: string;
  source_key: string;
  name: string;
  proxy_url: string;
  username: string | null;
  password: string | null;
  no_proxy_hosts: string[];
  enabled: boolean;
  existing_proxy_id: string;
  errors: string[];
}

export type CodexImportStatus = "pending" | "importing" | "imported" | "failed";

export interface CodexCredentialImportDraft {
  id: string;
  source: CodexImportSource;
  source_name: string;
  selected: boolean;
  label: string;
  email: string;
  account_id: string;
  id_token: string;
  access_token: string;
  refresh_token: string;
  source_proxy_key: string;
  proxy_id: string;
  weight: string;
  quota_threshold_percent: string;
  enabled: boolean;
  warnings: string[];
  errors: string[];
  status: CodexImportStatus;
  result_message: string;
}

export interface CodexImportParseResult {
  credentials: CodexCredentialImportDraft[];
  proxies: ImportedProxyDraft[];
  errors: string[];
}

const MAX_CREDENTIALS = 500;
const MAX_PROXIES = 500;
const PROXY_PROTOCOLS = new Set([
  "http:",
  "https:",
  "socks4:",
  "socks4a:",
  "socks5:",
  "socks5h:",
]);

type JsonRecord = Record<string, unknown>;

interface ParseAccumulator {
  credentials: CodexCredentialImportDraft[];
  proxies: ImportedProxyDraft[];
  errors: string[];
}

interface ParsedProxyUrl {
  proxyUrl: string;
  username: string | null;
  password: string | null;
  error: string | null;
}

export function parseCodexImportDocuments(
  documents: CodexImportDocument[],
): CodexImportParseResult {
  const result: ParseAccumulator = {
    credentials: [],
    proxies: [],
    errors: [],
  };

  documents.forEach((document, documentIndex) => {
    const trimmed = document.content.trim();
    if (!trimmed) {
      result.errors.push(`${document.name}: Import document is empty.`);
      return;
    }
    let value: unknown;
    try {
      value = JSON.parse(trimmed) as unknown;
    } catch {
      result.errors.push(`${document.name}: JSON parsing failed.`);
      return;
    }
    parseValue(
      value,
      document.name,
      `document-${documentIndex + 1}`,
      result,
    );
  });

  if (result.credentials.length > MAX_CREDENTIALS) {
    result.errors.push(`Import supports at most ${MAX_CREDENTIALS} credentials at a time.`);
    result.credentials = result.credentials.slice(0, MAX_CREDENTIALS);
  }
  if (result.proxies.length > MAX_PROXIES) {
    result.errors.push(`Import supports at most ${MAX_PROXIES} proxies at a time.`);
    result.proxies = result.proxies.slice(0, MAX_PROXIES);
  }
  markDuplicateCredentials(result.credentials);
  return result;
}

function parseValue(
  value: unknown,
  sourceName: string,
  documentKey: string,
  result: ParseAccumulator,
): void {
  if (Array.isArray(value)) {
    value.forEach((item, index) =>
      parseValue(item, sourceName, `${documentKey}-item-${index + 1}`, result),
    );
    return;
  }
  if (!isRecord(value)) {
    result.errors.push(`${sourceName}: Unsupported JSON root.`);
    return;
  }

  const payload = isRecord(value.data) ? value.data : value;
  if (stringValue(payload.type) === "ai-gateway-codex-credentials") {
    parseNativeBundle(payload, sourceName, documentKey, result);
    return;
  }
  if (Array.isArray(payload.accounts)) {
    parseSub2ApiBundle(payload, sourceName, documentKey, result);
    return;
  }
  parsePortableCredential(payload, sourceName, documentKey, "cpa", result);
}

function parseNativeBundle(
  payload: JsonRecord,
  sourceName: string,
  documentKey: string,
  result: ParseAccumulator,
): void {
  const proxyKeyMap = parseProxyArray(
    payload.proxies,
    sourceName,
    documentKey,
    "ai_gateway",
    result,
  );
  if (!Array.isArray(payload.credentials)) {
    result.errors.push(`${sourceName}: Native bundle has no credentials array.`);
    return;
  }
  payload.credentials.forEach((value, index) => {
    if (!isRecord(value)) {
      result.errors.push(`${sourceName}: Credential ${index + 1} is not an object.`);
      return;
    }
    const rawProxyKey = stringValue(value.proxy_key);
    const sourceProxyKey = rawProxyKey ? (proxyKeyMap.get(rawProxyKey) ?? "") : "";
    result.credentials.push(
      normalizeCredential({
        value,
        source: "ai_gateway",
        sourceName,
        itemKey: `${documentKey}-credential-${index + 1}`,
        sourceProxyKey,
        defaultLabel: `Codex credential ${index + 1}`,
      }),
    );
  });
}

function parseSub2ApiBundle(
  payload: JsonRecord,
  sourceName: string,
  documentKey: string,
  result: ParseAccumulator,
): void {
  const proxyKeyMap = parseProxyArray(
    payload.proxies,
    sourceName,
    documentKey,
    "sub2api",
    result,
  );
  const accounts = Array.isArray(payload.accounts) ? payload.accounts : [];
  let accepted = 0;
  accounts.forEach((value, index) => {
    if (!isRecord(value)) return;
    const platform = stringValue(value.platform).toLowerCase();
    const accountType = stringValue(value.type).toLowerCase();
    if ((platform && platform !== "openai") || (accountType && accountType !== "oauth")) {
      return;
    }
    const credentials = isRecord(value.credentials) ? value.credentials : value;
    if (!hasCredentialMaterial(credentials)) return;
    accepted += 1;
    const rawProxyKey = stringValue(value.proxy_key);
    const sourceProxyKey = rawProxyKey ? (proxyKeyMap.get(rawProxyKey) ?? "") : "";
    result.credentials.push(
      normalizeCredential({
        value: {
          ...credentials,
          name: stringValue(value.name),
        },
        source: "sub2api",
        sourceName,
        itemKey: `${documentKey}-account-${index + 1}`,
        sourceProxyKey,
        defaultLabel: `Sub2API Codex account ${accepted}`,
      }),
    );
  });
  if (accepted === 0) {
    result.errors.push(`${sourceName}: No OpenAI OAuth credentials were found.`);
  }
}

function parsePortableCredential(
  value: JsonRecord,
  sourceName: string,
  documentKey: string,
  preferredSource: CodexImportSource,
  result: ParseAccumulator,
): void {
  const providerType = stringValue(value.type).toLowerCase();
  const source =
    providerType === "codex" || hasCredentialMaterial(value)
      ? preferredSource
      : "generic";
  if (!hasCredentialMaterial(value)) {
    result.errors.push(`${sourceName}: No Codex token fields were found.`);
    return;
  }
  let sourceProxyKey = "";
  const rawProxyUrl = firstString(value, [["proxy_url"], ["proxyUrl"]]);
  if (rawProxyUrl) {
    const proxy = normalizeProxy({
      value: {
        name: `${firstString(value, [["email"], ["name"]]) || "Codex"} proxy`,
        proxy_url: rawProxyUrl,
      },
      source,
      sourceName,
      sourceKey: `${documentKey}-inline-proxy`,
      fallbackName: `${sourceName} proxy`,
    });
    sourceProxyKey = proxy.source_key;
    result.proxies.push(proxy);
  }
  result.credentials.push(
    normalizeCredential({
      value,
      source,
      sourceName,
      itemKey: `${documentKey}-credential`,
      sourceProxyKey,
      defaultLabel: fileStem(sourceName) || "Codex credential",
    }),
  );
}

function parseProxyArray(
  value: unknown,
  sourceName: string,
  documentKey: string,
  source: CodexImportSource,
  result: ParseAccumulator,
): Map<string, string> {
  const keyMap = new Map<string, string>();
  if (!Array.isArray(value)) return keyMap;
  value.forEach((proxyValue, index) => {
    if (!isRecord(proxyValue)) {
      result.errors.push(`${sourceName}: Proxy ${index + 1} is not an object.`);
      return;
    }
    const rawKey =
      firstString(proxyValue, [["proxy_key"], ["proxyKey"], ["id"]]) ||
      `proxy-${index + 1}`;
    const sourceKey = `${documentKey}-proxy-${rawKey}`;
    const proxy = normalizeProxy({
      value: proxyValue,
      source,
      sourceName,
      sourceKey,
      fallbackName: `Imported proxy ${index + 1}`,
    });
    keyMap.set(rawKey, sourceKey);
    result.proxies.push(proxy);
  });
  return keyMap;
}

function normalizeProxy({
  value,
  source,
  sourceName,
  sourceKey,
  fallbackName,
}: {
  value: JsonRecord;
  source: CodexImportSource;
  sourceName: string;
  sourceKey: string;
  fallbackName: string;
}): ImportedProxyDraft {
  const protocol = firstString(value, [["protocol"]]);
  const host = firstString(value, [["host"]]);
  const port = numberValue(value.port);
  const assembledUrl =
    protocol && host && port !== null ? `${protocol}://${formatHost(host)}:${port}` : "";
  const parsed = parseProxyUrl(
    firstString(value, [["proxy_url"], ["proxyUrl"]]) || assembledUrl,
  );
  const username =
    firstString(value, [["username"]]) || parsed.username;
  const password =
    firstString(value, [["password"]]) || parsed.password;
  const errors = parsed.error ? [parsed.error] : [];
  const status = stringValue(value.status).toLowerCase();
  return {
    id: sourceKey,
    source,
    source_name: sourceName,
    source_key: sourceKey,
    name: firstString(value, [["name"]]) || fallbackName,
    proxy_url: parsed.proxyUrl,
    username: username || null,
    password: password || null,
    no_proxy_hosts: stringArray(value.no_proxy_hosts),
    enabled: booleanValue(
      value.enabled,
      status !== "inactive" && status !== "disabled",
    ),
    existing_proxy_id: "",
    errors,
  };
}

function normalizeCredential({
  value,
  source,
  sourceName,
  itemKey,
  sourceProxyKey,
  defaultLabel,
}: {
  value: JsonRecord;
  source: CodexImportSource;
  sourceName: string;
  itemKey: string;
  sourceProxyKey: string;
  defaultLabel: string;
}): CodexCredentialImportDraft {
  const accessToken = firstString(value, [
    ["tokens", "access_token"],
    ["tokens", "accessToken"],
    ["access_token"],
    ["accessToken"],
    ["token"],
  ]);
  const refreshToken = firstString(value, [
    ["tokens", "refresh_token"],
    ["tokens", "refreshToken"],
    ["refresh_token"],
    ["refreshToken"],
  ]);
  const idToken = firstString(value, [
    ["tokens", "id_token"],
    ["tokens", "idToken"],
    ["id_token"],
    ["idToken"],
  ]);
  const claims = decodeJwtClaims(idToken || accessToken);
  const email =
    firstString(value, [["email"], ["user", "email"]]) ||
    claimEmail(claims);
  const accountId =
    firstString(value, [
      ["chatgpt_account_id"],
      ["chatgptAccountId"],
      ["account_id"],
      ["accountId"],
      ["account", "id"],
      ["account", "account_id"],
      ["account", "chatgpt_account_id"],
    ]) || claimAccountId(claims);
  const label =
    firstString(value, [["label"], ["name"], ["user", "name"]]) ||
    email ||
    accountId ||
    defaultLabel;
  const warnings: string[] = [];
  const errors: string[] = [];
  if (!idToken) warnings.push("ID token is missing; identity will be read from the access token.");
  if (!accountId) warnings.push("Account ID will be derived from the token during validation.");
  if (!label.trim()) errors.push("Label is required.");
  if (!accessToken) errors.push("Access token is required.");
  if (!refreshToken) errors.push("Refresh token is required.");
  const weight = positiveIntegerString(value.weight, "100");
  const threshold = boundedPercentString(value.quota_threshold_percent, "95");
  if (weight === null) errors.push("Weight must be a positive integer.");
  if (threshold === null) errors.push("Quota threshold must be from 1 to 100.");
  return {
    id: itemKey,
    source,
    source_name: sourceName,
    selected: errors.length === 0,
    label: label.slice(0, 100),
    email,
    account_id: accountId,
    id_token: idToken,
    access_token: accessToken,
    refresh_token: refreshToken,
    source_proxy_key: sourceProxyKey,
    proxy_id: "",
    weight: weight ?? stringValue(value.weight),
    quota_threshold_percent: threshold ?? stringValue(value.quota_threshold_percent),
    enabled: booleanValue(value.enabled, true),
    warnings,
    errors,
    status: "pending",
    result_message: "",
  };
}

function parseProxyUrl(value: string): ParsedProxyUrl {
  if (!value) {
    return {
      proxyUrl: "",
      username: null,
      password: null,
      error: "Proxy URL is required.",
    };
  }
  try {
    const url = new URL(value);
    if (
      !PROXY_PROTOCOLS.has(url.protocol) ||
      !url.hostname ||
      (url.pathname !== "" && url.pathname !== "/") ||
      url.search ||
      url.hash
    ) {
      throw new Error("unsupported proxy URL");
    }
    const username = url.username ? decodeURIComponent(url.username) : null;
    const password = url.password ? decodeURIComponent(url.password) : null;
    url.username = "";
    url.password = "";
    return {
      proxyUrl: `${url.protocol}//${url.host}`,
      username,
      password,
      error: null,
    };
  } catch {
    return {
      proxyUrl: value,
      username: null,
      password: null,
      error: "Enter a valid HTTP(S) or SOCKS proxy URL.",
    };
  }
}

function markDuplicateCredentials(credentials: CodexCredentialImportDraft[]): void {
  const seen = new Map<string, number>();
  credentials.forEach((credential, index) => {
    const key = credential.account_id
      ? `account:${credential.account_id}`
      : credential.refresh_token
        ? `refresh:${credential.refresh_token}`
        : credential.access_token
          ? `access:${credential.access_token}`
          : "";
    if (!key) return;
    const prior = seen.get(key);
    if (prior === undefined) {
      seen.set(key, index);
      return;
    }
    credential.errors = [
      ...credential.errors,
      `Duplicate of import row ${prior + 1}.`,
    ];
    credential.selected = false;
  });
}

function hasCredentialMaterial(value: JsonRecord): boolean {
  return Boolean(
    firstString(value, [
      ["tokens", "access_token"],
      ["tokens", "accessToken"],
      ["access_token"],
      ["accessToken"],
      ["token"],
      ["refresh_token"],
      ["refreshToken"],
      ["id_token"],
      ["idToken"],
    ]),
  );
}

function firstString(value: JsonRecord, paths: string[][]): string {
  for (const path of paths) {
    let current: unknown = value;
    for (const segment of path) {
      if (!isRecord(current)) {
        current = undefined;
        break;
      }
      current = current[segment];
    }
    const candidate = stringValue(current);
    if (candidate) return candidate;
  }
  return "";
}

function decodeJwtClaims(token: string): JsonRecord | null {
  if (!token) return null;
  const parts = token.split(".");
  if (parts.length !== 3 || !parts[1]) return null;
  try {
    const normalized = parts[1].replace(/-/g, "+").replace(/_/g, "/");
    const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
    const binary = atob(padded);
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    const decoded = JSON.parse(new TextDecoder().decode(bytes)) as unknown;
    return isRecord(decoded) ? decoded : null;
  } catch {
    return null;
  }
}

function claimEmail(claims: JsonRecord | null): string {
  if (!claims) return "";
  return firstString(claims, [
    ["email"],
    ["https://api.openai.com/profile", "email"],
  ]);
}

function claimAccountId(claims: JsonRecord | null): string {
  if (!claims) return "";
  return firstString(claims, [
    ["https://api.openai.com/auth", "chatgpt_account_id"],
  ]);
}

function positiveIntegerString(value: unknown, fallback: string): string | null {
  if (value === undefined || value === null || value === "") return fallback;
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? String(parsed) : null;
}

function boundedPercentString(value: unknown, fallback: string): string | null {
  if (value === undefined || value === null || value === "") return fallback;
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= 1 && parsed <= 100
    ? String(parsed)
    : null;
}

function stringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => stringValue(item))
    .filter((item) => Boolean(item));
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function numberValue(value: unknown): number | null {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

function stringValue(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function fileStem(value: string): string {
  return value.replace(/\.json$/i, "").trim();
}

function formatHost(host: string): string {
  return host.includes(":") && !host.startsWith("[") ? `[${host}]` : host;
}
