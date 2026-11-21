import { useMemo, useState } from "react";
import { Laptop, Monitor, Smartphone, Tablet, Terminal, type LucideIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { AccountPage } from "@/features/profile/account-page";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { useRevokeOtherSessions, useRevokeSession, useSessions } from "@/features/sessions/api";
import {
  describeSessionDevice,
  type SessionDeviceKind,
} from "@/features/sessions/session-display";
import type { ConsoleSession } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { formatDateTime } from "@/lib/dates";

type RevokeTarget = { kind: "session"; session: ConsoleSession } | { kind: "others" };

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
      <dd className="text-xs" title={value}>
        {value}
      </dd>
    </div>
  );
}

function SessionRow({
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
    <article className="flex flex-col gap-3 border-b py-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-3">
          <span className="grid size-8 shrink-0 place-items-center rounded-md bg-muted text-muted-foreground">
            <DeviceIcon className="size-4" aria-hidden />
          </span>
          <div className="min-w-0">
            <h3 className="flex flex-wrap items-center gap-2 text-sm font-medium">
              {device.label}
              {!active &&
                (session.state === "revoked" ? (
                  <Badge variant="destructive">{t("Revoked session")}</Badge>
                ) : (
                  <Badge variant="secondary">{t("Expired session")}</Badge>
                ))}
            </h3>
          </div>
        </div>
        {active && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => onRevoke(session)}
            disabled={disabled}
            aria-label={
              session.is_current
                ? t("Sign out this device")
                : t("Sign out {device}", { device: device.label })
            }
          >
            {session.is_current ? t("Sign out this device") : t("Sign out session")}
          </Button>
        )}
      </div>
      <dl className="grid gap-2 sm:grid-cols-3 sm:gap-4 sm:pl-11">
        <SessionFact label={t("Signed in at")} value={formatDateTime(session.created_at)} />
        <SessionFact label={t("Last refreshed")} value={formatDateTime(session.last_seen_at)} />
        <SessionFact
          label={active ? t("Expires") : endedLabel}
          value={formatDateTime(active ? session.expires_at : endedAt)}
        />
      </dl>
    </article>
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
  const current = activeSessions.find((session) => session.is_current);
  const others = activeSessions.filter((session) => !session.is_current);
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
    const device = describeSessionDevice(target.session.user_agent, t("Unknown browser"));
    return target.session.is_current
      ? {
          title: t("Sign out this device?"),
          description: t(
            "This is your current Console session. You will return to the sign-in page.",
          ),
        }
      : {
          title: t("Sign out {device}?", { device: device.label }),
          description: t("This immediately ends that browser session. It can sign in again later."),
        };
  })();

  return (
    <AccountPage>
      <AsyncResource
        isLoading={isLoading}
        error={error}
        isEmpty={sessions?.length === 0}
        emptyTitle="No login sessions"
      >
        {current && (
          <section>
            <h2 className="text-lg font-semibold">{t("Current device")}</h2>
            <SessionRow
              session={current}
              disabled={mutationPending}
              onRevoke={(session) => setTarget({ kind: "session", session })}
            />
          </section>
        )}
        <section>
          <header className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="flex items-center gap-2 text-lg font-semibold">
              {t("Other devices")}
              <span className="text-sm font-normal text-muted-foreground">{others.length}</span>
            </h2>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setTarget({ kind: "others" })}
              disabled={!others.length || mutationPending}
            >
              {t("Sign out other devices")}
            </Button>
          </header>
          {others.length ? (
            others.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                disabled={mutationPending}
                onRevoke={(nextTarget) => setTarget({ kind: "session", session: nextTarget })}
              />
            ))
          ) : (
            <p className="py-3 text-sm text-muted-foreground">
              {t("No other devices are signed in.")}
            </p>
          )}
        </section>

        {sessionHistory.length > 0 ? (
          <Collapsible open={historyOpen} onOpenChange={setHistoryOpen}>
            <header className="flex items-center justify-between gap-2">
              <h2 className="flex items-center gap-2 text-lg font-semibold">
                {t("Session history")}
                <span className="text-sm font-normal text-muted-foreground">
                  {sessionHistory.length}
                </span>
              </h2>
              <CollapsibleTrigger render={<Button variant="ghost" size="sm" />}>
                {historyOpen ? t("Hide history") : t("Show history")}
              </CollapsibleTrigger>
            </header>
            <CollapsibleContent>
              {sessionHistory.map((session) => (
                <SessionRow
                  key={session.id}
                  session={session}
                  disabled
                  onRevoke={() => undefined}
                />
              ))}
            </CollapsibleContent>
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
    </AccountPage>
  );
}
