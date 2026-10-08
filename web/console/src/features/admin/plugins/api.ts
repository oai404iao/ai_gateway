import { apiGet, apiGetDetail, consoleFetch } from "@/api/client";
import { readApiError } from "@/api/errors";
import type {
  MutationResponse, PluginAuthorization, PluginJob, PluginSettingsInput,
  PluginSettingsView, PluginStateInput, PluginView,
} from "@/api/types";

export const pluginPath = (id: string) => `/plugins/${encodeURIComponent(id)}`;
export const pluginKeys = {
  all: ["plugins"] as const,
  detail: (id: string) => ["plugins", id] as const,
  settings: (id: string) => ["plugins", id, "settings"] as const,
};

export const listPlugins = (signal?: AbortSignal) => apiGet<PluginView[]>("/plugins", signal);
export const getPlugin = (id: string, signal?: AbortSignal) => apiGetDetail<PluginView>(pluginPath(id), signal);
export const getPluginSettings = (id: string, signal?: AbortSignal) =>
  apiGetDetail<PluginSettingsView>(`${pluginPath(id)}/settings`, signal);
export const authorizePluginManagement = (password: string) =>
  write<PluginAuthorization>("/plugins/reauth", "POST", "", { body: { password } });
export const getPluginJob = (id: string, signal?: AbortSignal) =>
  apiGet<PluginJob>(`/plugins/jobs/${encodeURIComponent(id)}`, signal);

async function write<T>(
  path: string, method: string, token: string,
  options: { body?: unknown; binaryBody?: Blob; ifMatch?: string } = {},
): Promise<T> {
  // Password verification and single-use native authorization must not be replayed by refresh.
  const response = await consoleFetch(path, { method, pluginAuthorization: token, skipAuthRetry: true, ...options });
  if (!response.ok) throw await readApiError(response);
  return response.json() as Promise<T>;
}

export const installPlugin = (file: File, token: string) =>
  write<PluginJob>("/plugins/install", "POST", token, { binaryBody: file });
export const discoverPlugins = (token: string) =>
  write<PluginJob>("/plugins/discover", "POST", token);
export const updatePluginState = (id: string, input: PluginStateInput, etag: string, token: string) =>
  write<MutationResponse>(`${pluginPath(id)}/state`, "PUT", token, { body: input, ifMatch: etag });
export const updatePluginSettings = (id: string, input: PluginSettingsInput, etag: string, token: string) =>
  write<MutationResponse>(`${pluginPath(id)}/settings`, "PUT", token, { body: input, ifMatch: etag });
export const deletePluginArtifact = (id: string, digest: string, etag: string, token: string) =>
  write<MutationResponse>(`${pluginPath(id)}/artifacts/${encodeURIComponent(digest)}`, "DELETE", token, { ifMatch: etag });
