import { Suspense } from "react";
import { Outlet, Link } from "react-router";
import { Spinner } from "@/components/ui/spinner";
import { LocaleToggle } from "@/components/shared/locale-toggle";
import { Brand } from "@/components/shared/brand";

export function AuthLayout() {
  return (
    <div className="relative flex min-h-svh flex-col items-center justify-center gap-8 bg-muted/30 p-6">
      <div className="absolute top-4 right-4">
        <LocaleToggle />
      </div>
      <Link to="/login">
        <Brand />
      </Link>
      <div className="w-full max-w-sm">
        <Suspense fallback={<Spinner className="mx-auto size-6" />}>
          <Outlet />
        </Suspense>
      </div>
    </div>
  );
}
