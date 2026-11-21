import { Suspense, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Link, Outlet, useLocation, useNavigate } from "react-router";
import {
  ArrowUpRight,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  LogOut,
  Monitor,
  Moon,
  Search,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
} from "lucide-react";
import { toast } from "sonner";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Brand } from "@/components/shared/brand";
import { LocaleToggle } from "@/components/shared/locale-toggle";
import { RouteFallback } from "@/components/shared/route-fallback";
import { logout } from "@/api/session";
import { useI18n } from "@/app/i18n";
import { useTheme } from "@/app/theme";
import { useIsMobile } from "@/hooks/use-mobile";
import { accountInitials } from "@/lib/account-initials";
import { roleLabel } from "@/lib/permissions";
import { useSession } from "@/lib/use-session";
import { cn } from "@/lib/utils";
import {
  accountPages,
  audit,
  globalLogs,
  keys,
  logs,
  managementGroups,
  navPages,
  models,
  overview,
  pageMatches,
  services,
  users,
  workspace,
  type NavItem,
  type NavGroup,
} from "./nav";
import { PageFinder } from "./page-finder";
import styles from "./console-layout.module.css";

function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const { t } = useI18n();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button variant="ghost" size="icon" aria-label={t("Toggle theme")} />}>
        <Sun className="dark:hidden" />
        <Moon className="hidden dark:block" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuGroup>
          <DropdownMenuRadioGroup
            value={theme}
            onValueChange={(value) => setTheme(value as "light" | "dark" | "system")}
          >
            <DropdownMenuRadioItem value="light">
              <Sun data-icon="inline-start" /> {t("Light")}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="dark">
              <Moon data-icon="inline-start" /> {t("Dark")}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="system">
              <Monitor data-icon="inline-start" /> {t("System default")}
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function UserMenu() {
  const { user } = useSession();
  const navigate = useNavigate();
  const { t } = useI18n();
  const initials = accountInitials(user?.display_name ?? user?.email ?? "");

  const onLogout = async () => {
    await logout();
    toast.success(t("Signed out"));
    navigate("/login", { replace: true });
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button variant="ghost" size="sm" aria-label={t("Account menu")} title={user?.display_name} />
        }
      >
        <Avatar className="size-8">
          <AvatarFallback className="bg-accent text-accent-foreground">{initials}</AvatarFallback>
        </Avatar>
        <ChevronDown data-icon="inline-end" className="opacity-60" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuGroup>
          <DropdownMenuLabel>
            <div className="flex flex-col">
              <span className="text-sm font-medium">{user?.display_name}</span>
              <span className="text-xs text-muted-foreground">{user?.email}</span>
              <span className="text-xs text-muted-foreground">{user ? roleLabel(user.role) : ""}</span>
            </div>
          </DropdownMenuLabel>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          {accountPages.map((item) => (
            <DropdownMenuItem key={item.path} render={<Link to={item.path} />}>
              <item.icon data-icon="inline-start" /> {t(item.label)}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={onLogout}>
            <LogOut data-icon="inline-start" /> {t("Sign out")}
          </DropdownMenuItem>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function PageLink({
  item,
  current,
  label = item.label,
}: {
  item: NavItem;
  current: boolean;
  label?: string;
}) {
  const { t } = useI18n();
  return (
    <Link to={item.path} className={styles.navLink} aria-current={current ? "page" : undefined}>
      <item.icon aria-hidden="true" /> <span>{t(label)}</span>
    </Link>
  );
}

function ManagementLinks({ items, pathname }: { items: (NavItem | NavGroup)[]; pathname: string }) {
  const { t } = useI18n();
  const activeGroup = navPages(items).find(({ item }) => pageMatches(item, pathname))?.parents[0];
  const [expanded, setExpanded] = useState(activeGroup);
  return items.map((entry) =>
    "items" in entry ? (
      <Collapsible
        key={entry.label}
        open={expanded === entry.label}
        onOpenChange={(open) => setExpanded(open ? entry.label : undefined)}
      >
        <CollapsibleTrigger className={styles.branch} data-current={entry.label === activeGroup || undefined}>
          <ChevronRight aria-hidden="true" />
          {t(entry.label)}
        </CollapsibleTrigger>
        <CollapsibleContent className={styles.children}>
          <ManagementLinks items={entry.items} pathname={pathname} />
        </CollapsibleContent>
      </Collapsible>
    ) : (
      <PageLink key={entry.path} item={entry} current={pageMatches(entry, pathname)} />
    ),
  );
}

function ManagementNavigation({ pathname, mobile }: { pathname: string; mobile: boolean }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const current = navPages(managementGroups).find(({ item }) => pageMatches(item, pathname));
  const links = <ManagementLinks items={managementGroups} pathname={pathname} />;
  return (
    <nav className={styles.local} aria-label={t("Management navigation")}>
      {mobile ? (
        <Collapsible open={open} onOpenChange={setOpen}>
          <CollapsibleTrigger className={styles.browse} aria-label={t("Browse management pages")}>
            <SlidersHorizontal aria-hidden="true" />
            <span>{t(current?.parents[0] ?? "Management")}</span>
            {current && (
              <>
                <ChevronRight aria-hidden="true" />
                <b>{t(current.item.label)}</b>
              </>
            )}
            <ChevronDown aria-hidden="true" />
          </CollapsibleTrigger>
          <CollapsibleContent className={styles.mobileTree}>{links}</CollapsibleContent>
        </Collapsible>
      ) : (
        <>
          <strong>
            <SlidersHorizontal aria-hidden="true" />
            {t("Management")}
          </strong>
          {links}
        </>
      )}
    </nav>
  );
}

function ManagementShortcuts() {
  const { t } = useI18n();
  return (
    <section className={styles.shortcuts} aria-label={t("Gateway management")}>
      <h2>
        <ShieldCheck aria-hidden="true" />
        {t("Gateway management")}
      </h2>
      <div>
        {[models, users, globalLogs, audit].map((item) => (
          <Link key={item.path} to={item.path}>
            {t(item.label)} <ArrowUpRight aria-hidden="true" />
          </Link>
        ))}
      </div>
    </section>
  );
}

export function ConsoleLayout() {
  const { user } = useSession();
  const { pathname } = useLocation();
  const { t } = useI18n();
  const mobile = useIsMobile();
  const [finding, setFinding] = useState(false);
  const main = useRef<HTMLElement>(null);
  const previous = useRef(pathname);
  const admin = user?.role === "admin";
  const managed = admin && pathname.startsWith("/admin/");
  const service = services.some((item) => pageMatches(item, pathname));
  useLayoutEffect(() => {
    if (previous.current === pathname) return;
    previous.current = pathname;
    setFinding(false);
    if (!main.current?.contains(document.activeElement)) {
      main.current?.focus({ preventScroll: true });
      window.scrollTo(0, 0);
    }
  }, [pathname]);
  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if (
        event.key.toLowerCase() !== "k" ||
        !(event.ctrlKey || event.metaKey) ||
        event.repeat ||
        document.querySelector('[role="dialog"], [role="alertdialog"]')
      )
        return;
      event.preventDefault();
      setFinding(true);
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, []);
  return (
    <div className={styles.shell}>
      <a className={styles.skip} href="#gateway-content">
        {t("Skip to content")}
      </a>
      <header className={styles.header}>
        <div className={cn(styles.width, styles.top)}>
          <Link to="/" aria-label={t("Gateway home")}>
            <Brand compact />
          </Link>
          <span className={styles.instance}>{t("Main gateway")}</span>
          <div className={styles.tools}>
            <Button
              variant={mobile ? "ghost" : "outline"}
              size="sm"
              onClick={() => setFinding(true)}
              aria-label={t("Find page")}
              aria-keyshortcuts="Control+K Meta+K"
            >
              <Search data-icon="inline-start" />
              <span className={styles.searchLabel}>{t("Find page")}</span>
              <kbd className={styles.kbd}>
                {/Mac|iPhone|iPad/.test(navigator.platform) ? "⌘ K" : "Ctrl K"}
              </kbd>
            </Button>
            <LocaleToggle />
            <ThemeToggle />
            <UserMenu />
          </div>
        </div>
        {!mobile && (
          <nav className={cn(styles.width, styles.topNav)} aria-label={t("Console navigation")}>
            {workspace.map((item) => (
              <PageLink key={item.path} item={item} current={pageMatches(item, pathname)} />
            ))}
            <PageLink item={services[0]} label="Services" current={service} />
            {admin && (
              <span className={styles.adminGate}>
                <PageLink
                  item={{ ...models, icon: SlidersHorizontal }}
                  label="Management"
                  current={managed}
                />
              </span>
            )}
          </nav>
        )}
      </header>
      {service && (
        <nav aria-label={t("Service navigation")} className={cn(styles.width, styles.services)}>
          {services.map((item) => (
            <PageLink item={item} key={item.path} current={pageMatches(item, pathname)} />
          ))}
        </nav>
      )}
      <div className={cn(styles.width, styles.stage, managed && styles.managed)}>
        {managed && <ManagementNavigation key={pathname} pathname={pathname} mobile={mobile} />}
        <main
          id="gateway-content"
          ref={main}
          tabIndex={-1}
          className="flex min-w-0 flex-col gap-6 outline-none"
        >
          <Suspense fallback={<RouteFallback />}>
            <Outlet />
          </Suspense>
          {admin && pathname === "/" && <ManagementShortcuts />}
        </main>
      </div>
      {mobile && (
        <nav className={styles.dock} aria-label={t("Mobile navigation")}>
          {[overview, logs, keys].map((item) => (
            <PageLink key={item.path} item={item} current={pageMatches(item, pathname)} />
          ))}
          <button
            type="button"
            className={styles.navLink}
            onClick={() => setFinding(true)}
            aria-haspopup="dialog"
          >
            <Ellipsis aria-hidden="true" />
            {t("More pages")}
          </button>
        </nav>
      )}
      <PageFinder admin={admin} open={finding} onOpenChange={setFinding} />
    </div>
  );
}
