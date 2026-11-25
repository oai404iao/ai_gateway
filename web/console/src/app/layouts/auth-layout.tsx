import { Suspense } from "react";
import { Outlet, Link } from "react-router";
import { Frame, FramePanel } from "@/components/ui/frame";
import { Spinner } from "@/components/ui/spinner";
import { LocaleToggle } from "@/components/shared/locale-toggle";
import { Brand } from "@/components/shared/brand";

export function AuthLayout() {
  return (
    <div className="flex min-h-svh flex-col bg-muted">
      <header className="flex items-center justify-between border-b bg-background px-5 py-4 sm:px-10">
        <Link to="/login" className="rounded-md">
          <Brand compact />
        </Link>
        <LocaleToggle />
      </header>
      <main className="flex flex-1 items-center justify-center px-4 pt-6 pb-14 sm:pb-24">
        <Frame className="w-full max-w-md">
          <FramePanel className="p-6 sm:p-8">
            <Suspense fallback={<Spinner className="mx-auto size-6" />}>
              <Outlet />
            </Suspense>
          </FramePanel>
        </Frame>
      </main>
    </div>
  );
}
