import { safeReturnPath } from "@/lib/page-navigation";

const RESOURCE_ID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function validResourceId(value: string | null): string | null {
  const candidate = value?.trim() ?? "";
  return RESOURCE_ID_PATTERN.test(candidate) ? candidate : null;
}

export function safeAdminReturnPath(
  value: string | null,
  fallback: string,
): string {
  return safeReturnPath(value, fallback);
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
