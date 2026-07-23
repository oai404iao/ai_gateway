import {
  KeyRound,
  ScrollText,
  Users,
  ShieldCheck,
  Boxes,
  GalleryVerticalEnd,
  Network,
  Route,
  FileText,
  RefreshCw,
  SlidersHorizontal,
  ChartNoAxesCombined,
  type LucideIcon,
} from "lucide-react";
import type { UserRole } from "@/api/types";

export interface NavItem {
  label: string;
  path: string;
  icon: LucideIcon;
  end?: boolean;
}

export interface NavSection {
  title: string;
  items: NavItem[];
  roles?: UserRole[];
}

export const NAV_SECTIONS: NavSection[] = [
  {
    title: "Personal",
    items: [
      { label: "Sessions", path: "/account/sessions", icon: ShieldCheck },
      { label: "API Keys", path: "/api-keys", icon: KeyRound },
      { label: "Request Logs", path: "/usage/request-logs", icon: ScrollText },
      { label: "Statistics", path: "/statistics", icon: ChartNoAxesCombined },
    ],
  },
  {
    title: "Administration",
    roles: ["admin"],
    items: [
      { label: "Users", path: "/admin/users", icon: Users },
      { label: "API Key Policies", path: "/admin/api-key-policies", icon: SlidersHorizontal },
      { label: "Models", path: "/admin/models", icon: Boxes },
      { label: "Price sync", path: "/admin/catalog", icon: GalleryVerticalEnd },
    ],
  },
  {
    title: "Routing",
    roles: ["admin"],
    items: [
      { label: "Channels", path: "/admin/routing/channels", icon: Network },
      { label: "Model Rules", path: "/admin/routing/model-rules", icon: Route },
    ],
  },
  {
    title: "Operations",
    roles: ["admin"],
    items: [
      { label: "Proxies", path: "/admin/network/proxies", icon: Network },
      { label: "Templates", path: "/admin/transforms/templates", icon: FileText },
      { label: "Request Logs", path: "/admin/request-logs", icon: ScrollText },
      { label: "Audit Logs", path: "/admin/audit-logs", icon: ShieldCheck },
      { label: "System settings", path: "/admin/system", icon: RefreshCw },
    ],
  },
];

export function visibleSections(role: UserRole | null): NavSection[] {
  if (!role) return [];
  return NAV_SECTIONS.filter(
    (section) => !section.roles || section.roles.includes(role),
  );
}
