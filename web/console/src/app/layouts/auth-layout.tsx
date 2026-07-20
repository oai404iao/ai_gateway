import { Suspense } from "react";
import { Outlet, Link } from "react-router";
import { Spinner } from "@/components/ui/spinner";
import { useI18n } from "@/app/i18n";
import { LocaleToggle } from "@/components/shared/locale-toggle";

export function AuthLayout() {
  const { t } = useI18n();
  return (
    <div className="relative flex min-h-svh flex-col items-center justify-center gap-8 bg-muted/30 p-6">
      <div className="absolute top-4 right-4">
        <LocaleToggle />
      </div>
      <Link to="/login" className="flex items-center gap-2">
        <div className="flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground">
          <span className="text-sm font-bold">AG</span>
        </div>
        <div className="flex flex-col">
          <span className="text-base font-semibold">AI Gateway</span>
          <span className="text-xs text-muted-foreground">{t("Console")}</span>
        </div>
      </Link>
      <div className="w-full max-w-sm">
        <Suspense fallback={<Spinner className="mx-auto size-6" />}>
          <Outlet />
        </Suspense>
      </div>
    </div>
  );
}
