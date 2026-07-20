import { Skeleton } from "@/components/ui/skeleton";

/** Suspense fallback shown while a route chunk loads. */
export function RouteFallback() {
  return (
    <div className="flex flex-col gap-4">
      <Skeleton className="h-8 w-64" />
      <Skeleton className="h-40 w-full" />
      <Skeleton className="h-40 w-full" />
    </div>
  );
}
