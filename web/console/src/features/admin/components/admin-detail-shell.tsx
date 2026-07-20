import { ArrowLeft } from "lucide-react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
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
}: AdminDetailShellProps) {
  const navigate = useNavigate();
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t(title)}
        description={description ? t(description) : undefined}
        actions={
          <Button variant="ghost" size="sm" onClick={() => navigate(backPath)}>
            <ArrowLeft data-icon="inline-start" /> {backLabel ?? t("Back")}
          </Button>
        }
      />
      <AsyncResource isLoading={isLoading} error={error}>
        {hasData ? (
          <>
            {detailCard}
            {editCard}
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
