import { useMemo, useState } from "react";
import {
  Laptop,
  LogOut,
  Monitor,
  Smartphone,
  Tablet,
  Terminal,
  type LucideIcon,
} from "lucide-react";
import { toast } from "sonner";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { EmptyState } from "@/components/shared/empty-state";
import {
  useRevokeOtherSessions,
  useRevokeSession,
  useSessions,
} from "@/features/sessions/api";
import {
  describeSessionDevice,
  shortSessionId,
  type SessionDeviceKind,
} from "@/features/sessions/session-display";
import { clearSession } from "@/api/session-store";
import type { ConsoleSession } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { formatDate, formatDateTime, formatRelative } from "@/lib/dates";

type RevokeTarget =
  | { kind: "session"; session: ConsoleSession }
  | { kind: "others" };

const DEVICE_ICONS: Record<SessionDeviceKind, LucideIcon> = {
  desktop: Laptop,
  mobile: Smartphone,
  tablet: Tablet,
  terminal: Terminal,
  unknown: Monitor,
};

function SessionFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="truncate font-medium" title={value}>
        {value}
      </dd>
    </div>
  );
}

function formatSessionTime(value: string): string {
  const relative = formatRelative(value);
  const absolute = formatDateTime(value);
  return relative === formatDate(value) ? absolute : `${relative} · ${absolute}`;
}

function SessionCard({
  session,
  disabled,
  onRevoke,
}: {
  session: ConsoleSession;
  disabled: boolean;
  onRevoke: (session: ConsoleSession) => void;
}) {
  const { t } = useI18n();
  const device = describeSessionDevice(session.user_agent, t("Unknown browser"));
  const DeviceIcon = DEVICE_ICONS[device.kind];
  const active = session.state === "active";
  const endedAt = session.state === "revoked" ? session.revoked_at : session.expires_at;
  const endedLabel = session.state === "revoked" ? t("Revoked at") : t("Expired at");

  return (
    <Card size="sm">
      <CardHeader>
        <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
          <div className="flex min-w-0 flex-col gap-1">
            <CardTitle className="flex min-w-0 items-center gap-2">
              <DeviceIcon className="size-4" aria-hidden />
              <span className="truncate">{device.label}</span>
            </CardTitle>
            <CardDescription>
              {t("Session {id}", { id: shortSessionId(session.id) })}
            </CardDescription>
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            {session.is_current ? (
              <Badge variant="info">{t("Current device")}</Badge>
            ) : null}
            {session.state === "active" ? (
              <Badge variant="success">{t("Active session")}</Badge>
            ) : session.state === "revoked" ? (
              <Badge variant="destructive">{t("Revoked session")}</Badge>
            ) : (
              <Badge variant="secondary">{t("Expired session")}</Badge>
            )}
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <dl className="grid gap-4 sm:grid-cols-3">
          <SessionFact
            label={t("Signed in at")}
            value={formatSessionTime(session.created_at)}
          />
          <SessionFact
            label={t("Last refreshed")}
            value={formatSessionTime(session.last_seen_at)}
          />
          <SessionFact
            label={active ? t("Expires") : endedLabel}
            value={formatDateTime(active ? session.expires_at : endedAt)}
          />
        </dl>
      </CardContent>
      {active ? (
        <CardFooter className="justify-end">
          <Button
            variant="destructive"
            size="sm"
            onClick={() => onRevoke(session)}
            disabled={disabled}
            aria-label={
              session.is_current
                ? t("Sign out this device")
                : t("Sign out {device}", { device: device.label })
            }
          >
            <LogOut data-icon="inline-start" />
            {session.is_current ? t("Sign out this device") : t("Sign out session")}
          </Button>
        </CardFooter>
      ) : null}
    </Card>
  );
}

export function SessionsPage() {
  const { t } = useI18n();
  const { data: sessions, isLoading, error } = useSessions();
  const revoke = useRevokeSession();
  const revokeOthers = useRevokeOtherSessions();
  const [target, setTarget] = useState<RevokeTarget | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);

  const activeSessions = useMemo(
    () =>
      (sessions ?? [])
        .filter((session) => session.state === "active")
        .sort(
          (left, right) =>
            Number(right.is_current) - Number(left.is_current) ||
            right.created_at.localeCompare(left.created_at),
        ),
    [sessions],
  );
  const sessionHistory = useMemo(
    () => (sessions ?? []).filter((session) => session.state !== "active"),
    [sessions],
  );
  const otherActiveCount = activeSessions.filter((session) => !session.is_current).length;
  const mutationPending = revoke.isPending || revokeOthers.isPending;

  const confirmRevoke = async () => {
    if (!target) return;
    const confirmedTarget = target;
    setTarget(null);
    try {
      if (confirmedTarget.kind === "others") {
        await revokeOthers.mutateAsync();
        toast.success(t("Other devices signed out"));
        return;
      }
      const { session } = confirmedTarget;
      await revoke.mutateAsync({ id: session.id, isCurrent: session.is_current });
      if (session.is_current) {
        toast.success(t("Signed out this device"));
        clearSession();
      } else {
        toast.success(t("Device signed out"));
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Sign out failed"));
    }
  };

  const confirmation = (() => {
    if (!target) {
      return { title: "", description: "" };
    }
    if (target.kind === "others") {
      return {
        title: t("Sign out other devices?"),
        description: t(
          "This keeps your current device signed in and immediately ends every other active session.",
        ),
      };
    }
    const device = describeSessionDevice(
      target.session.user_agent,
      t("Unknown browser"),
    );
    return target.session.is_current
      ? {
          title: t("Sign out this device?"),
          description: t(
            "This is your current Console session. You will return to the sign-in page.",
          ),
        }
      : {
          title: t("Sign out {device}?", { device: device.label }),
          description: t(
            "This immediately ends that browser session. It can sign in again later.",
          ),
        };
  })();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Login sessions")}
        description={t(
          "Review the browsers signed in to your Console account and end sessions you do not recognize.",
        )}
      />

      <AsyncResource
        isLoading={isLoading}
        error={error}
        isEmpty={sessions?.length === 0}
        emptyTitle="No login sessions"
        emptyDescription="No browser sessions are recorded for this account."
      >
        <Card>
          <CardHeader>
            <CardTitle>{t("Active sessions")}</CardTitle>
            <CardDescription>
              {t(
                "Session activity is updated when the browser refreshes its sign-in credential.",
              )}
            </CardDescription>
            <CardAction>
              <Badge variant="secondary">{activeSessions.length}</Badge>
            </CardAction>
          </CardHeader>
          <CardContent>
            {activeSessions.length > 0 ? (
              <div className="flex flex-col gap-3">
                {activeSessions.map((session) => (
                  <SessionCard
                    key={session.id}
                    session={session}
                    disabled={mutationPending}
                    onRevoke={(nextTarget) =>
                      setTarget({ kind: "session", session: nextTarget })
                    }
                  />
                ))}
              </div>
            ) : (
              <EmptyState
                title={t("No active sessions")}
                description={t("No browser currently has an active Console login.")}
                className="py-10"
              />
            )}
          </CardContent>
          <CardFooter className="justify-end">
            <Button
              variant="destructive"
              size="sm"
              onClick={() => setTarget({ kind: "others" })}
              disabled={otherActiveCount === 0 || mutationPending}
            >
              <LogOut data-icon="inline-start" />
              {t("Sign out other devices")}
            </Button>
          </CardFooter>
        </Card>

        {sessionHistory.length > 0 ? (
          <Collapsible open={historyOpen} onOpenChange={setHistoryOpen}>
            <Card>
              <CardHeader>
                <CardTitle>{t("Session history")}</CardTitle>
                <CardDescription>
                  {t("Expired and revoked sessions are kept here for security review.")}
                </CardDescription>
                <CardAction className="flex items-center gap-2">
                  <Badge variant="secondary">{sessionHistory.length}</Badge>
                  <CollapsibleTrigger
                    render={<Button variant="ghost" size="sm" />}
                  >
                    {historyOpen ? t("Hide history") : t("Show history")}
                  </CollapsibleTrigger>
                </CardAction>
              </CardHeader>
              <CollapsibleContent>
                <CardContent>
                  <div className="flex flex-col gap-3">
                    {sessionHistory.map((session) => (
                      <SessionCard
                        key={session.id}
                        session={session}
                        disabled
                        onRevoke={() => undefined}
                      />
                    ))}
                  </div>
                </CardContent>
              </CollapsibleContent>
            </Card>
          </Collapsible>
        ) : null}
      </AsyncResource>

      <ConfirmDialog
        open={Boolean(target)}
        onOpenChange={(open) => !open && setTarget(null)}
        title={confirmation.title}
        description={confirmation.description}
        confirmLabel={t("Sign out")}
        destructive
        onConfirm={confirmRevoke}
      />
    </div>
  );
}
