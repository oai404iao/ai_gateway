import {
  Activity,
  Boxes,
  ChartNoAxesCombined,
  FileText,
  Gauge,
  KeyRound,
  LayoutDashboard,
  ListTree,
  Network,
  Puzzle,
  Route,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  TicketCheck,
  Trophy,
  UserRound,
  Users,
  UsersRound,
  type LucideIcon,
} from "lucide-react";
import {
  healthSections,
  maintenance,
  transportSections,
  type SettingsSection,
} from "@/features/admin/system/settings-sections";

export interface NavItem {
  label: string;
  path: string;
  icon: LucideIcon;
  related?: string[];
}
export const overview = { label: "Overview", path: "/", icon: LayoutDashboard };
export const usage = { label: "Usage", path: "/statistics", icon: ChartNoAxesCombined };
export const logs = { label: "Request logs", path: "/usage/request-logs", icon: ListTree };
export const keys = { label: "API keys", path: "/api-keys", icon: KeyRound };
export const workspace = [overview, usage, logs, keys];
export const services = [
  { label: "Channel group status", path: "/channel-group-status", icon: Activity },
  { label: "Codex quotas", path: "/codex-quotas", icon: Gauge },
  { label: "Spend leaderboard", path: "/leaderboard", icon: Trophy },
];
export const accountPages = [
  { label: "Profile", path: "/account", icon: UserRound },
  { label: "Personal settings", path: "/account/settings", icon: Settings2 },
  { label: "Login sessions", path: "/account/sessions", icon: ShieldCheck },
];
export const models = {
  label: "Model configuration",
  path: "/admin/models",
  icon: Route,
  related: ["/admin/routing/operation-rules"],
};
export const users = { label: "Users", path: "/admin/users", icon: Users };
export const globalLogs = { label: "Global request logs", path: "/admin/request-logs", icon: ListTree };
export const audit = { label: "Audit Logs", path: "/admin/audit-logs", icon: ShieldCheck };
export interface NavGroup {
  label: string;
  items: (NavGroup | NavItem)[];
}
function settingsLink(section: { id: SettingsSection; label: string }): NavItem {
  return { label: section.label, path: `/admin/system/${section.id}`, icon: Settings2 };
}
export const managementGroups: NavGroup[] = [
  {
    label: "Model routing",
    items: [
      models,
      { label: "Upstream accesses", path: "/admin/routing/accesses", icon: Network },
      {
        label: "Channel configuration",
        path: "/admin/routing/channels",
        icon: Boxes,
        related: ["/admin/routing/groups", "/admin/routing/logical-channels", "/admin/routing/capabilities"],
      },
      { label: "Upstream credentials", path: "/admin/routing/upstream-credentials", icon: KeyRound },
      { label: "Proxies", path: "/admin/network/proxies", icon: Network },
      { label: "Templates", path: "/admin/transforms/templates", icon: FileText },
    ],
  },
  {
    label: "Members and access",
    items: [
      users,
      { label: "User Groups", path: "/admin/user-groups", icon: UsersRound },
      { label: "Registration Codes", path: "/admin/registration-invitation-codes", icon: TicketCheck },
      { label: "API Key Policies", path: "/admin/api-key-policies", icon: SlidersHorizontal },
    ],
  },
  {
    label: "Operations",
    items: [
      globalLogs,
      { label: "Cost statistics", path: "/admin/cost-statistics", icon: ChartNoAxesCombined },
      audit,
      { label: "System load", path: "/admin/system-load", icon: Gauge },
    ],
  },
  {
    label: "System settings",
    items: [
      { label: "Connections and protocols", items: transportSections.map(settingsLink) },
      { label: "Routing and health", items: healthSections.map(settingsLink) },
      settingsLink(maintenance),
      { label: "Plugins", path: "/admin/plugins", icon: Puzzle },
    ],
  },
];
export function pageMatches(item: NavItem, path: string) {
  return [item.path, ...(item.related ?? [])].some((base) => path === base || path.startsWith(`${base}/`));
}

export function navPages(
  items: (NavItem | NavGroup)[],
  parents: string[] = [],
): { item: NavItem; parents: string[] }[] {
  return items.flatMap((entry) =>
    "items" in entry ? navPages(entry.items, [...parents, entry.label]) : [{ item: entry, parents }],
  );
}

export function finderPages(admin: boolean) {
  return navPages([
    { label: "Main gateway", items: workspace },
    { label: "Services", items: services },
    { label: "My account", items: accountPages },
    ...(admin ? [{ label: "Management", items: managementGroups }] : []),
  ]);
}
