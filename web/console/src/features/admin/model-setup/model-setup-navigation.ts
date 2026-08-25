const RESOURCE_ID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export const MODEL_SETUP_PATH = "/admin/model-setup";

export function validResourceId(value: string | null): string | null {
  const candidate = value?.trim() ?? "";
  return RESOURCE_ID_PATTERN.test(candidate) ? candidate : null;
}

export function safeAdminReturnPath(
  value: string | null,
  fallback: string,
): string {
  if (!value?.startsWith("/") || value.startsWith("//")) return fallback;

  try {
    const base = new URL("https://console.invalid");
    const parsed = new URL(value, base);
    if (parsed.origin !== base.origin || !parsed.pathname.startsWith("/admin/")) {
      return fallback;
    }
    return `${parsed.pathname}${parsed.search}${parsed.hash}`;
  } catch {
    return fallback;
  }
}

export function adminPath(
  path: string,
  params: Record<string, string | null | undefined>,
): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value) search.set(key, value);
  }
  const query = search.toString();
  return query ? `${path}?${query}` : path;
}
