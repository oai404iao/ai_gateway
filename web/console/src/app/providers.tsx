import { useEffect, useRef, useState } from "react";
import { MotionConfig } from "motion/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { ThemeProvider } from "@/app/theme";
import { I18nProvider } from "@/app/i18n-provider";
import { sessionScope } from "@/api/session-store";
import { refreshAccessToken } from "@/api/client";
import { useSession } from "@/lib/use-session";
import { ErrorBoundary } from "@/components/shared/error-boundary";
import { Spinner } from "@/components/ui/spinner";

function createSessionCache(scope: string) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: 30_000, gcTime: 5 * 60_000, refetchOnWindowFocus: false },
      mutations: { retry: false },
    },
  });
  return { scope, client };
}

/** Restores the session once on load using the HttpOnly refresh cookie. */
function SessionGate({ children }: { children: React.ReactNode }) {
  const { status, user } = useSession();
  const scope = sessionScope(user);
  const [cache, setCache] = useState(() => createSessionCache(scope));
  useEffect(() => () => cache.client.clear(), [cache]);
  const booted = useRef(false);

  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    void refreshAccessToken();
  }, []);

  // Swap before descendants render so even a fresh cached /me result cannot cross accounts.
  if (scope !== cache.scope) {
    setCache(createSessionCache(scope));
    return null;
  }
  if (status === "loading") {
    return (
      <div className="flex min-h-svh items-center justify-center">
        <Spinner className="size-6" />
      </div>
    );
  }
  return <QueryClientProvider client={cache.client}>{children}</QueryClientProvider>;
}

export function AppProviders({ children }: { children: React.ReactNode }) {
  return (
    <ErrorBoundary>
      <MotionConfig reducedMotion="user" transition={{ duration: 0.15 }}>
        <I18nProvider>
          <ThemeProvider>
            <TooltipProvider>
              <SessionGate>{children}</SessionGate>
              <Toaster richColors closeButton />
            </TooltipProvider>
          </ThemeProvider>
        </I18nProvider>
      </MotionConfig>
    </ErrorBoundary>
  );
}
