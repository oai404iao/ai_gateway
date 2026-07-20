import { AlertCircle } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Skeleton } from "@/components/ui/skeleton";
import { ApiError } from "@/api/errors";
import { EmptyState } from "@/components/shared/empty-state";
import { translate, useI18n } from "@/app/i18n";

interface AsyncResourceProps {
  isLoading: boolean;
  error: unknown;
  isEmpty?: boolean;
  emptyTitle?: string;
  emptyDescription?: string;
  /** Rendered when data is ready and non-empty. */
  children: React.ReactNode;
}

export function AsyncResource({
  isLoading,
  error,
  isEmpty,
  emptyTitle,
  emptyDescription,
  children,
}: AsyncResourceProps) {
  const { t } = useI18n();
  if (isLoading) {
    return (
      <div className="flex flex-col gap-3">
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-full" />
      </div>
    );
  }
  if (error) {
    return <ErrorAlert error={error} />;
  }
  if (isEmpty) {
    return (
      <EmptyState
        title={t(emptyTitle ?? "Nothing here yet")}
        description={t(emptyDescription ?? "There are no items to show.")}
        className="py-12"
      />
    );
  }
  return <>{children}</>;
}

export function ErrorAlert({ error }: { error: unknown }) {
  const { t } = useI18n();
  const message = errorMessage(error);
  return (
    <Alert variant="destructive">
      <AlertCircle data-icon="inline-start" />
      <AlertTitle>{t("Request failed")}</AlertTitle>
      <AlertDescription>{message}</AlertDescription>
    </Alert>
  );
}

function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return error.code === error.message
      ? translate("Console rejected the request ({code}).", { code: error.code })
      : error.message;
  }
  if (error instanceof Error) return error.message;
  return translate("An unexpected error occurred.");
}
