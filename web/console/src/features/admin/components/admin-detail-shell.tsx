import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { useI18n } from "@/app/i18n";

interface AdminDetailShellProps {
  title: string;
  description?: string;
  backPath: string;
  backLabel?: string;
  isLoading: boolean;
  error: unknown;
  hasData: boolean;
  /** Shown when data is present: read-only facts. */
  detailCard?: React.ReactNode;
  /** Shown when data is present: edit form. */
  editCard?: React.ReactNode;
  /** Shown when data is present: destructive actions. */
  dangerZone?: React.ReactNode;
  headerActions?: React.ReactNode;
  navigationGuard?: React.ReactNode;
  actionBar?: React.ReactNode;
  embedded?: boolean;
}

export function AdminDetailShell({
  title,
  description,
  backPath,
  backLabel,
  isLoading,
  error,
  hasData,
  detailCard,
  editCard,
  dangerZone,
  headerActions,
  navigationGuard,
  actionBar,
  embedded = false,
}: AdminDetailShellProps) {
  const { t } = useI18n();
  if (embedded) {
    return (
      <>
        {navigationGuard}
        <AsyncResource isLoading={isLoading} error={error}>
          {hasData ? editCard : null}
        </AsyncResource>
      </>
    );
  }
  return (
    <div className="flex min-w-0 flex-col gap-6">
      {navigationGuard}
      <PageHeader
        title={t(title)}
        description={description ? t(description) : undefined}
        backTo={backPath}
        backLabel={backLabel}
        actions={headerActions}
      />
      <AsyncResource isLoading={isLoading} error={error}>
        {hasData ? (
          <>
            {editCard && detailCard ? (
              <div className="grid min-w-0 items-start gap-5 xl:grid-cols-[minmax(0,1fr)_18rem]">
                <div className="min-w-0">{editCard}</div>
                <aside className="flex min-w-0 flex-col gap-4 xl:sticky xl:top-20">
                  {detailCard}
                </aside>
              </div>
            ) : <>{editCard}{detailCard}</>}
            {actionBar}
            {dangerZone ? (
              <>
                <Separator />
                <Card>
                  <CardHeader>
                    <CardTitle className="text-destructive">{t("Danger zone")}</CardTitle>
                    <CardDescription>
                      {t("These actions are permanent and audited.")}
                    </CardDescription>
                  </CardHeader>
                  <CardContent>{dangerZone}</CardContent>
                </Card>
              </>
            ) : null}
          </>
        ) : null}
      </AsyncResource>
    </div>
  );
}
