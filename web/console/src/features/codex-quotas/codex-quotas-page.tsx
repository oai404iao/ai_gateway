import { useMemo, useState } from "react";
import { History } from "lucide-react";
import type {
  CodexQuotaResetReason,
  SelfCodexQuotaCredentialView,
  SelfCodexQuotaWindowPeriod,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { EmptyState } from "@/components/shared/empty-state";
import { PageHeader } from "@/components/shared/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import {
  useOwnCodexQuotas,
  useOwnCodexQuotaWindowHistory,
} from "@/features/codex-quotas/api";
import { formatDateTime } from "@/lib/dates";
import { formatEstimatedQuotaTotal, formatUsd } from "@/lib/formatters";

function windowDuration(seconds: number | null): string {
  if (seconds === null) return "—";
  if (seconds % 86_400 === 0) return `${seconds / 86_400}d`;
  if (seconds % 3_600 === 0) return `${seconds / 3_600}h`;
  return `${seconds}s`;
}

function resetReasonLabel(
  reason: CodexQuotaResetReason | null,
): string {
  if (reason === "natural") return "Natural reset";
  if (reason === "manual") return "Manual reset credit";
  if (reason === "openai_official") return "OpenAI official reset";
  return "Current period";
}

function resetReasonVariant(
  reason: CodexQuotaResetReason | null,
): "success" | "warning" | "info" | "secondary" {
  if (reason === "manual") return "warning";
  if (reason === "openai_official") return "info";
  if (reason === "natural") return "secondary";
  return "success";
}

function QuotaWindow({
  label,
  percent,
  seconds,
  resetAt,
  costAmount,
}: {
  label: string;
  percent: number | null;
  seconds: number | null;
  resetAt: string | null;
  costAmount: string | null;
}) {
  const { t } = useI18n();
  return (
    <div className="flex min-w-44 flex-col gap-1">
      <div className="flex items-center justify-between gap-3 text-xs">
        <span>{t(label)}</span>
        <span className="tabular-nums">
          {percent === null ? "—" : `${percent}%`}
        </span>
      </div>
      <Progress value={percent ?? 0} />
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>{windowDuration(seconds)}</span>
        <span>
          {resetAt
            ? t("Resets {time}", { time: formatDateTime(resetAt) })
            : "—"}
        </span>
      </div>
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">{t("Period spend")}</span>
        <span className="font-medium tabular-nums">
          {formatUsd(costAmount)}
        </span>
      </div>
      <div className="flex items-center justify-between gap-3 text-xs">
        <span className="text-muted-foreground">
          {t("Estimated total quota")}
        </span>
        <span className="font-medium tabular-nums">
          {formatEstimatedQuotaTotal(costAmount, percent)}
        </span>
      </div>
    </div>
  );
}

function QuotaHistoryDialog({
  credential,
  onClose,
}: {
  credential: SelfCodexQuotaCredentialView | null;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const history = useOwnCodexQuotaWindowHistory(credential?.id ?? "");
  const primary = (history.data?.periods ?? []).filter(
    (period) => period.window_kind === "primary",
  );
  const secondary = (history.data?.periods ?? []).filter(
    (period) => period.window_kind === "secondary",
  );

  return (
    <Dialog open={credential !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-5xl">
        <DialogHeader>
          <DialogTitle>
            {t("Quota window history for {name}", {
              name: credential?.name ?? "",
            })}
          </DialogTitle>
          <DialogDescription>
            {t(
              "This view is read-only and contains quota windows plus the provider-reported subscription tier. Period spend covers all priced requests routed through the credential, not only your requests.",
            )}
          </DialogDescription>
        </DialogHeader>
        <AsyncResource
          isLoading={history.isLoading}
          error={history.error}
          isEmpty={(history.data?.periods.length ?? 0) === 0}
          emptyTitle={t("No quota window history")}
          emptyDescription={t("History begins with the first stored quota observation.")}
        >
          <Tabs defaultValue="primary">
            <TabsList>
              <TabsTrigger value="primary">
                {t("Primary window")} ({primary.length})
              </TabsTrigger>
              <TabsTrigger value="secondary">
                {t("Secondary window")} ({secondary.length})
              </TabsTrigger>
            </TabsList>
            <TabsContent value="primary">
              <QuotaWindowPeriodsTable periods={primary} />
            </TabsContent>
            <TabsContent value="secondary">
              <QuotaWindowPeriodsTable periods={secondary} />
            </TabsContent>
          </Tabs>
        </AsyncResource>
      </DialogContent>
    </Dialog>
  );
}

function QuotaWindowPeriodsTable({
  periods,
}: {
  periods: SelfCodexQuotaWindowPeriod[];
}) {
  const { t } = useI18n();
  if (periods.length === 0) {
    return (
      <EmptyState
        title={t("No periods for this window")}
        description={t("The provider has not reported this quota window yet.")}
        className="py-10"
      />
    );
  }
  return (
    <div className="overflow-x-auto rounded-xl border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{t("Period")}</TableHead>
            <TableHead>{t("Usage")}</TableHead>
            <TableHead>{t("Period spend")}</TableHead>
            <TableHead>{t("Estimated total quota")}</TableHead>
            <TableHead>{t("Ended by")}</TableHead>
            <TableHead>{t("Last observed")}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {periods.map((period) => (
            <TableRow
              key={`${period.window_kind}:${period.started_at}:${period.scheduled_reset_at}`}
            >
              <TableCell className="min-w-64 align-top">
                <div className="flex flex-col gap-1">
                  <span className="font-medium">
                    {formatDateTime(period.started_at)}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {period.ended_at
                      ? t("Ended {time}", {
                          time: formatDateTime(period.ended_at),
                        })
                      : t("Scheduled reset {time}", {
                          time: formatDateTime(period.scheduled_reset_at),
                        })}
                  </span>
                  <Badge variant="outline" className="w-fit">
                    {windowDuration(period.window_seconds)}
                  </Badge>
                </div>
              </TableCell>
              <TableCell className="align-top tabular-nums">
                {period.initial_used_percent}% → {period.last_used_percent}%
              </TableCell>
              <TableCell className="align-top tabular-nums">
                {formatUsd(period.cost_amount)}
              </TableCell>
              <TableCell className="align-top tabular-nums">
                {formatEstimatedQuotaTotal(
                  period.cost_amount,
                  period.last_used_percent,
                )}
              </TableCell>
              <TableCell className="align-top">
                <Badge variant={resetReasonVariant(period.reset_reason)}>
                  {t(resetReasonLabel(period.reset_reason))}
                </Badge>
              </TableCell>
              <TableCell className="align-top">
                {formatDateTime(period.last_observed_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

export function CodexQuotasPage() {
  const { t } = useI18n();
  const quotas = useOwnCodexQuotas();
  const [historyCredential, setHistoryCredential] =
    useState<SelfCodexQuotaCredentialView | null>(null);
  const groups = useMemo(() => {
    const grouped = new Map<string, SelfCodexQuotaCredentialView[]>();
    for (const credential of quotas.data ?? []) {
      const current = grouped.get(credential.channel_group_id) ?? [];
      grouped.set(credential.channel_group_id, [...current, credential]);
    }
    return [...grouped.entries()];
  }, [quotas.data]);

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Codex quotas")}
        description={t(
          "Read-only quota windows and credential-wide spend for Codex credential groups granted by your user group. Spend includes every caller and both Responses and Images. Estimated total quota divides current period spend by the most recently provider-reported used percentage.",
        )}
      />
      <AsyncResource
        isLoading={quotas.isLoading}
        error={quotas.error}
        isEmpty={(quotas.data?.length ?? 0) === 0}
        emptyTitle={t("No Codex quota access")}
        emptyDescription={t(
          "Your user group has not been granted access to any Codex quota groups.",
        )}
      >
        <div className="flex flex-col gap-6">
          {groups.map(([groupId, credentials]) => (
            <Card key={groupId}>
              <CardHeader>
                <CardTitle>{t("Channel group")}</CardTitle>
                <CardDescription>{groupId}</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="overflow-x-auto rounded-xl border">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t("Name")}</TableHead>
                        <TableHead>{t("Subscription")}</TableHead>
                        <TableHead>{t("Primary window")}</TableHead>
                        <TableHead>{t("Secondary window")}</TableHead>
                        <TableHead>{t("Last checked")}</TableHead>
                        <TableHead className="text-right">
                          {t("Quota history")}
                        </TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {credentials.map((credential) => (
                        <TableRow key={credential.id}>
                          <TableCell>{credential.name}</TableCell>
                          <TableCell>
                            <Badge variant="secondary">
                              {credential.plan_type ?? t("Unknown")}
                            </Badge>
                          </TableCell>
                          <TableCell>
                            <QuotaWindow
                              label="Primary window"
                              percent={credential.primary_used_percent}
                              seconds={credential.primary_window_seconds}
                              resetAt={credential.primary_reset_at}
                              costAmount={
                                credential.primary_window_cost_amount
                              }
                            />
                          </TableCell>
                          <TableCell>
                            <QuotaWindow
                              label="Secondary window"
                              percent={credential.secondary_used_percent}
                              seconds={credential.secondary_window_seconds}
                              resetAt={credential.secondary_reset_at}
                              costAmount={
                                credential.secondary_window_cost_amount
                              }
                            />
                          </TableCell>
                          <TableCell>
                            {formatDateTime(credential.quota_checked_at)}
                          </TableCell>
                          <TableCell className="text-right">
                            <Button
                              size="sm"
                              variant="outline"
                              aria-label={t("View quota history for {name}", {
                                name: credential.name,
                              })}
                              onClick={() => setHistoryCredential(credential)}
                            >
                              <History data-icon="inline-start" />
                              {t("Quota history")}
                            </Button>
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      </AsyncResource>
      <QuotaHistoryDialog
        credential={historyCredential}
        onClose={() => setHistoryCredential(null)}
      />
    </div>
  );
}
