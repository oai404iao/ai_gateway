import { ApiError, NetworkError, readApiError } from "@/api/errors";
import { clearSession, getAccessToken, getSession, sessionScope, setSession } from "@/api/session-store";

const API_PREFIX = "/console/v1";
const REFRESH_PATH = `${API_PREFIX}/auth/refresh`;

// Single-flight refresh: concurrent 401s share one refresh promise so the
// rotating refresh cookie is exercised once per failure burst.
let refreshPromise: Promise<string | null> | null = null;

interface RequestOptions {
  method?: string;
  body?: unknown;
  binaryBody?: Blob;
  pluginAuthorization?: string;
  /** ETag captured from the GET detail response, sent as If-Match on a mutation. */
  ifMatch?: string;
  /** Suppresses the automatic 401 refresh+retry path (used by refresh itself). */
  skipAuthRetry?: boolean;
  signal?: AbortSignal;
}

function buildHeaders(options: RequestOptions): HeadersInit {
  const headers: Record<string, string> = {};
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  if (options.binaryBody !== undefined) headers["Content-Type"] = "application/octet-stream";
  if (options.pluginAuthorization) headers["X-Plugin-Authorization"] = options.pluginAuthorization;
  const token = getAccessToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  if (options.ifMatch) headers["If-Match"] = options.ifMatch;
  return headers;
}

async function parseJson(response: Response): Promise<unknown> {
  if (response.status === 204) return null;
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

/**
 * Low-level Console fetch with Bearer injection and single-flight refresh.
 * Returns the raw Response so callers can read headers (ETag) when needed.
 */
export async function consoleFetch(
  path: string,
  options: RequestOptions = {},
): Promise<Response> {
  const url = path.startsWith("http") ? path : `${API_PREFIX}${path}`;
  const scope = sessionScope(getSession().user);
  const init: RequestInit = {
    method: options.method ?? "GET",
    headers: buildHeaders(options),
    credentials: "include",
    signal: options.signal,
  };
  if (options.body !== undefined) init.body = JSON.stringify(options.body);
  if (options.binaryBody !== undefined) init.body = options.binaryBody;

  let response: Response;
  try {
    response = await fetch(url, init);
  } catch (error) {
    throw new NetworkError(
      error instanceof Error ? error.message : "network request failed",
      error,
    );
  }

  if (response.status === 401 && !options.skipAuthRetry && scope === sessionScope(getSession().user)) {
    const refreshed = await refreshAccessToken();
    // A shared refresh cookie can now belong to a different account in another tab.
    if (refreshed && scope === sessionScope(getSession().user)) {
      return consoleFetch(path, { ...options, skipAuthRetry: true });
    }
  }

  return response;
}

/** Ensures the response is ok or throws an ApiError. */
async function ensureOk(response: Response): Promise<void> {
  if (!response.ok) throw await readApiError(response);
}

/** GET a list or detail body. */
export async function apiGet<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await consoleFetch(path, { signal });
  await ensureOk(response);
  return (await parseJson(response)) as T;
}

/** GET a detail resource and capture its ETag for optimistic-concurrency mutations. */
export async function apiGetDetail<T>(
  path: string,
  signal?: AbortSignal,
): Promise<{ data: T; etag: string }> {
  const response = await consoleFetch(path, { signal });
  await ensureOk(response);
  const etag = response.headers.get("ETag");
  if (!etag) {
    throw new ApiError(
      response.status,
      "missing_etag",
      "resource response did not include an ETag",
    );
  }
  return { data: (await parseJson(response)) as T, etag };
}

/** POST/PUT/PATCH/DELETE helper that returns the parsed body. */
export async function apiSend<T>(
  path: string,
  method: string,
  body?: unknown,
  options: { ifMatch?: string; signal?: AbortSignal } = {},
): Promise<T> {
  const response = await consoleFetch(path, {
    method,
    body,
    ifMatch: options.ifMatch,
    signal: options.signal,
  });
  await ensureOk(response);
  return (await parseJson(response)) as T;
}

export function apiPost<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  return apiSend<T>(path, "POST", body, { signal });
}

export function apiPut<T>(
  path: string,
  body: unknown,
  ifMatch: string,
  signal?: AbortSignal,
): Promise<T> {
  return apiSend<T>(path, "PUT", body, { ifMatch, signal });
}

export function apiDelete(path: string, signal?: AbortSignal): Promise<void> {
  return consoleFetch(path, { method: "DELETE", signal }).then(ensureOk);
}

export function buildQuery(params: object): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") {
      search.set(key, String(value));
    }
  }
  const query = search.toString();
  return query ? `?${query}` : "";
}

/**
 * Refreshes the access token using the HttpOnly refresh cookie.
 * Concurrent callers share one rotation; a failure clears the session.
 */
export function refreshAccessToken(): Promise<string | null> {
  if (refreshPromise) return refreshPromise;
  const original = getSession();
  const stillCurrent = () => getAccessToken() === original.accessToken && getSession().status === original.status;
  refreshPromise = (async () => {
    try {
      const response = await fetch(REFRESH_PATH, {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
      });
      if (!response.ok) {
        if (stillCurrent()) clearSession();
        return null;
      }
      const data = (await response.json()) as {
        access_token: string;
        user: import("@/api/types").ConsoleUser;
      };
      if (!stillCurrent()) return null;
      setSession({
        status: "authenticated",
        accessToken: data.access_token,
        user: data.user,
      });
      return data.access_token;
    } catch {
      if (stillCurrent()) clearSession();
      return null;
    } finally {
      refreshPromise = null;
    }
  })();
  return refreshPromise;
}
