import { useEffect, useRef, useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { ThemeProvider } from "@/app/theme";
import { refreshAccessToken } from "@/api/client";
import { useSession } from "@/lib/use-session";
import { ErrorBoundary } from "@/components/shared/error-boundary";
import { Spinner } from "@/components/ui/spinner";

/** Restores the session once on load using the HttpOnly refresh cookie. */
function SessionGate({ children }: { children: React.ReactNode }) {
  const { status } = useSession();
  const booted = useRef(false);

  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    void refreshAccessToken();
  }, []);

  if (status === "loading") {
    return (
      <div className="flex min-h-svh items-center justify-center">
        <Spinner className="size-6" />
      </div>
    );
  }
  return <>{children}</>;
}

export function AppProviders({ children }: { children: React.ReactNode }) {
  // A fresh QueryClient per mount keeps component tests isolated: a previous
  // test's cached query data must not satisfy a page that should refetch.
  const [queryClient] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            retry: false,
            staleTime: 30_000,
            gcTime: 5 * 60_000,
            refetchOnWindowFocus: false,
          },
          mutations: { retry: false },
        },
      }),
  );
  return (
    <ErrorBoundary>
      <ThemeProvider>
        <QueryClientProvider client={queryClient}>
          <TooltipProvider>
            <SessionGate>{children}</SessionGate>
            <Toaster richColors closeButton />
          </TooltipProvider>
        </QueryClientProvider>
      </ThemeProvider>
    </ErrorBoundary>
  );
}
