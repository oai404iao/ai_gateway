import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useI18n } from "@/app/i18n";
import { ArrowLeft } from "lucide-react";
import { NavigationLink } from "./navigation-link";
import { returnPathLabel } from "@/lib/page-navigation";

interface PageHeaderProps {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  backTo?: string;
  backLabel?: string;
  embedded?: boolean;
}

export function PageHeader({ title, description, actions, backTo, backLabel, embedded = false }: PageHeaderProps) {
  const { t } = useI18n();
  const Heading = embedded ? "h2" : "h1";
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {backTo ? (
        <NavigationLink to={backTo} variant="ghost" className="self-start">
          <ArrowLeft data-icon="inline-start" /> {t(backLabel ?? returnPathLabel(backTo))}
        </NavigationLink>
      ) : null}
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
      <div className="flex min-w-0 flex-col gap-1">
        <Heading className={embedded ? "break-words text-lg font-semibold tracking-tight" : "break-words text-2xl font-semibold tracking-tight"}>{t(title)}</Heading>
        {description ? (
          <p className="text-sm text-muted-foreground">{t(description)}</p>
        ) : null}
      </div>
      {actions ? (
        <div className="flex flex-wrap items-center justify-end gap-2">
          {actions}
        </div>
      ) : null}
      </div>
    </div>
  );
}

export function PageHeaderSkeleton() {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <Skeleton className="h-5 w-40" />
        </CardTitle>
      </CardHeader>
      <CardContent>
        <Skeleton className="h-4 w-full" />
      </CardContent>
    </Card>
  );
}
