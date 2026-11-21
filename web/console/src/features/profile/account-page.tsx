import type { ReactNode } from "react";
import { NavLink } from "react-router";
import { useI18n } from "@/app/i18n";
import { accountPages } from "@/app/layouts/nav";
import { PageHeader } from "@/components/shared/page-header";

export function AccountPage({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  return (
    <div className="flex w-full max-w-3xl min-w-0 flex-col gap-6">
      <PageHeader title="Account settings" />
      <nav aria-label={t("Account settings")} className="flex gap-4 border-b sm:gap-6">
        {accountPages.map(({ path, label }) => (
          <NavLink
            key={path}
            to={path}
            end
            className="-mb-px border-b-2 border-transparent pb-2 text-sm text-muted-foreground hover:text-foreground aria-[current=page]:border-primary aria-[current=page]:font-medium aria-[current=page]:text-foreground"
          >
            {t(label)}
          </NavLink>
        ))}
      </nav>
      {children}
    </div>
  );
}
