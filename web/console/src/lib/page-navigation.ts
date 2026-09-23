import { useLocation, useSearchParams } from "react-router";

export function safeReturnPath(value: string | null, fallback: string): string {
  if (!value?.startsWith("/") || value.startsWith("//") ||
    [...value].some((character) => character === "\\" || character.charCodeAt(0) <= 0x20)) {
    return fallback;
  }
  try {
    const base = new URL("https://console.invalid");
    const parsed = new URL(value, base);
    const allowed = fallback.startsWith("/admin/") ? "/admin/" : "/api-keys";
    if (parsed.origin !== base.origin ||
      !(parsed.pathname === allowed || parsed.pathname.startsWith(
        allowed.endsWith("/") ? allowed : `${allowed}/`,
      ))) return fallback;
    return `${parsed.pathname}${parsed.search}${parsed.hash}`;
  } catch {
    return fallback;
  }
}

export function withReturnTo(path: string, returnTo: string): string {
  const url = new URL(path, "https://console.invalid");
  const origin = new URL(returnTo, "https://console.invalid");
  if (url.pathname === origin.pathname) {
    for (const [key, value] of origin.searchParams) {
      if (!url.searchParams.has(key)) url.searchParams.set(key, value);
    }
  } else {
    url.searchParams.set("returnTo", returnTo);
  }
  return `${url.pathname}${url.search}${url.hash}`;
}

export function useReturnPath(fallback: string): string {
  const [params] = useSearchParams();
  return safeReturnPath(params.get("returnTo"), fallback);
}

export function usePageOrigin(): string {
  const { pathname, search, hash } = useLocation();
  return `${pathname}${search}${hash}`;
}

export function returnPathLabel(path: string): string {
  const url = new URL(path, "https://console.invalid");
  if (url.pathname === "/admin/models") return url.searchParams.get("view") === "prices" ? "Back to price sync" : "Back to client models";
  if (url.pathname === "/admin/routing/channels") return url.searchParams.get("view") === "groups" ? "Back to channel groups" : "Back to channels";
  const parents = [
    ["/admin/routing/logical-channels", "Back to channels", "Back to channel"],
    ["/admin/routing/groups", "Back to channel groups", "Back to channel group"],
    ["/admin/routing/upstream-credentials", "Back to credentials", "Back to credential"],
    ["/admin/routing/accesses", "Back to accesses", "Back to access"],
    ["/admin/routing/capabilities", "Back to capabilities", "Back to capability"],
    ["/admin/routing/operation-rules", "Back to operation rules", "Back to operation rule"],
    ["/admin/models", "Back to client models", "Back to model"],
    ["/admin/users", "Back to users", "Back to user"],
    ["/admin/user-groups", "Back to user groups", "Back to user group"],
    ["/admin/api-key-policies", "Back to API key policies", "Back to API key policy"],
    ["/admin/registration-invitation-codes", "Back to registration codes", "Back to registration code"],
    ["/admin/network/proxies", "Back to proxies", "Back to proxy"],
    ["/admin/transforms/templates", "Back to templates", "Back to template"],
    ["/admin/codex-sharing", "Back to sharing groups", "Back to sharing group"],
    ["/api-keys", "Back to API keys", "Back to API key"],
  ] as const;
  for (const [parent, listLabel, detailLabel] of parents) {
    if (url.pathname === parent) return listLabel;
    if (url.pathname.startsWith(`${parent}/`)) return detailLabel;
  }
  return "Back";
}
