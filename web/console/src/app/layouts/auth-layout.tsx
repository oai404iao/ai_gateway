import { Suspense } from "react";
import { Outlet, Link } from "react-router";
import { Spinner } from "@/components/ui/spinner";
import { LocaleToggle } from "@/components/shared/locale-toggle";
import { Brand } from "@/components/shared/brand";

export function AuthLayout() {
  return (
    <div className="flex min-h-svh flex-col bg-linear-to-br from-primary/5 via-background to-background">
      <header className="flex items-center justify-between px-5 py-4 sm:px-10">
        <Link to="/login" className="rounded-md">
          <Brand compact />
        </Link>
        <LocaleToggle />
      </header>
      <main className="flex flex-1 items-center justify-center px-6 pt-6 pb-14 sm:pb-24">
        <div className="w-full max-w-sm">
          <svg
            viewBox="0 0 108 48"
            fill="none"
            aria-hidden="true"
            className="mb-4 w-24 text-primary/70"
          >
            <g stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path opacity=".45" d="M5 6h21c13 0 17 18 30 18h8M5 42h21c13 0 17-18 30-18" />
              <path d="M5 24h57m16 0h24" />
              <rect x="62" y="16" width="16" height="16" rx="5" className="fill-primary/10" />
            </g>
            <circle cx="70" cy="24" r="2.5" fill="currentColor" />
          </svg>
          <Suspense fallback={<Spinner className="mx-auto size-6" />}>
            <Outlet />
          </Suspense>
        </div>
      </main>
    </div>
  );
}
