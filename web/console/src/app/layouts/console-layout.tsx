import { Suspense } from "react";
import { NavLink, Outlet, useNavigate } from "react-router";
import { LogOut, Moon, Sun, Monitor, ChevronDown, User } from "lucide-react";
import { toast } from "sonner";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
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
import { Button } from "@/components/ui/button";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { useSession } from "@/lib/use-session";
import { useTheme } from "@/app/theme";
import { useI18n } from "@/app/i18n";
import { logout } from "@/api/session";
import { roleLabel } from "@/lib/permissions";
import { visibleSections } from "@/app/layouts/nav";
import { RouteFallback } from "@/components/shared/route-fallback";
import { LocaleToggle } from "@/components/shared/locale-toggle";
import { Brand } from "@/components/shared/brand";

function BrandHeader() {
  return (
    <SidebarHeader>
      <Brand compact collapseInSidebar className="px-2 py-3" />
    </SidebarHeader>
  );
}

function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  const { t } = useI18n();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button variant="ghost" size="icon" aria-label={t("Toggle theme")} />
        }
      >
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
  const initials = (user?.display_name ?? user?.email ?? "?")
    .split(/\s+/)
    .map((part) => part[0])
    .filter(Boolean)
    .slice(0, 2)
    .join("")
    .toUpperCase();

  const onLogout = async () => {
    await logout();
    toast.success(t("Signed out"));
    navigate("/login", { replace: true });
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button variant="ghost" size="sm" />}>
        <Avatar className="size-7">
          <AvatarFallback>{initials}</AvatarFallback>
        </Avatar>
        <span className="hidden text-sm sm:inline-flex">{user?.display_name}</span>
        <ChevronDown data-icon="inline-end" className="opacity-60" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuGroup>
          <DropdownMenuLabel>
            <div className="flex flex-col">
              <span className="text-sm font-medium">{user?.display_name}</span>
              <span className="text-xs text-muted-foreground">{user?.email}</span>
              <span className="text-xs text-muted-foreground">
                {user ? roleLabel(user.role) : ""}
              </span>
            </div>
          </DropdownMenuLabel>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuItem onClick={() => navigate("/account")}>
            <User data-icon="inline-start" /> {t("Profile")}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onLogout}>
            <LogOut data-icon="inline-start" /> {t("Sign out")}
          </DropdownMenuItem>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function ConsoleLayout() {
  const { user } = useSession();
  const { t } = useI18n();
  const sections = visibleSections(user?.role ?? null);

  return (
    <SidebarProvider>
      <Sidebar collapsible="icon">
        <BrandHeader />
        <SidebarContent>
          {sections.map((section) => (
            <SidebarGroup key={section.title}>
              <SidebarGroupLabel>{t(section.title)}</SidebarGroupLabel>
              <SidebarGroupContent>
                <SidebarMenu>
                  {section.items.map((item) => (
                    <SidebarMenuItem key={item.path}>
                      <SidebarMenuButton asChild>
                        <NavLink to={item.path} end={item.end}>
                          <>
                            <item.icon />
                            <span>{t(item.label)}</span>
                          </>
                        </NavLink>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </SidebarGroupContent>
            </SidebarGroup>
          ))}
        </SidebarContent>
        <SidebarFooter />
      </Sidebar>
      <SidebarInset>
        <header className="sticky top-0 z-10 flex h-14 items-center gap-2 border-b bg-background/95 px-4 backdrop-blur">
          <SidebarTrigger />
          <div className="ml-auto flex items-center gap-1">
            <LocaleToggle />
            <ThemeToggle />
            <UserMenu />
          </div>
        </header>
        <main className="flex-1 p-4 md:p-6">
          <div className="mx-auto flex max-w-6xl flex-col gap-6">
            <Suspense fallback={<RouteFallback />}>
              <Outlet />
            </Suspense>
          </div>
        </main>
      </SidebarInset>
    </SidebarProvider>
  );
}
