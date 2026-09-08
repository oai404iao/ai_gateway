import { ArrowLeft } from "lucide-react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { useI18n } from "@/app/i18n";
import { ConfigurationNav } from "@/features/admin/model-setup/configuration-workbench";
import type { ConfigurationLens } from "@/features/admin/model-setup/configuration-graph";

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
  /** Additional primary actions shown in the page header before Back. */
  headerActions?: React.ReactNode;
  configurationLens?: ConfigurationLens;
  navigationGuard?: React.ReactNode;
  onBack?: () => void;
  saving?: boolean;
  actionBar?: React.ReactNode;
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
  configurationLens,
  navigationGuard,
  onBack,
  saving,
  actionBar,
}: AdminDetailShellProps) {
  const navigate = useNavigate();
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-6">
      {navigationGuard}
      {configurationLens && <ConfigurationNav lens={configurationLens} />}
      <PageHeader
        title={t(title)}
        description={description ? t(description) : undefined}
        actions={
          <>
            {headerActions}
            <Button variant="ghost" size="sm" disabled={saving} onClick={onBack ?? (() => navigate(backPath))}>
              <ArrowLeft data-icon="inline-start" /> {configurationLens && backPath.includes("selected=") ? t("Back to workbench") : backLabel ?? t("Back")}
            </Button>
          </>
        }
      />
      <AsyncResource isLoading={isLoading} error={error}>
        {hasData ? (
          <>
            {configurationLens && detailCard ? (
              <div className="grid min-w-0 items-start gap-5 xl:grid-cols-[minmax(0,1fr)_18rem]">
                <div className="min-w-0">{editCard}</div>
                <aside className="flex min-w-0 flex-col gap-4 xl:sticky xl:top-20">
                  {detailCard}
                  <Card size="sm">
                    <CardHeader><CardTitle>{t("Editing one resource")}</CardTitle>
                      <CardDescription>{t("Related resources are not changed automatically. Return to the workbench to inspect the complete request path.")}</CardDescription>
                    </CardHeader>
                    <CardContent><Button variant="outline" size="sm" disabled={saving} onClick={onBack ?? (() => navigate(backPath))}>{t("Back to workbench")}</Button></CardContent>
                  </Card>
                </aside>
              </div>
            ) : <>{detailCard}{editCard}</>}
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
            {actionBar}
          </>
        ) : null}
      </AsyncResource>
    </div>
  );
}
