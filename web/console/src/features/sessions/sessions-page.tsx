import { useState } from "react";
import { toast } from "sonner";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useRevokeSession, useSessions } from "@/features/sessions/api";
import type { ConsoleSession } from "@/api/types";
import { formatDateTime, formatRelative } from "@/lib/dates";

export function SessionsPage() {
  const { data: sessions, isLoading, error } = useSessions();
  const revoke = useRevokeSession();
  const [target, setTarget] = useState<ConsoleSession | null>(null);

  const confirmRevoke = async () => {
    if (!target) return;
    const id = target.id;
    setTarget(null);
    try {
      await revoke.mutateAsync(id);
      toast.success("Session revoked");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Revoke failed");
    }
  };

  const columns: Column<ConsoleSession>[] = [
    {
      key: "created",
      header: "Created",
      render: (session) => (
        <span className="flex flex-col">
          <span>{formatDateTime(session.created_at)}</span>
          <span className="text-xs text-muted-foreground">
            {formatRelative(session.created_at)}
          </span>
        </span>
      ),
    },
    {
      key: "last_seen",
      header: "Last seen",
      render: (session) =>
        session.last_seen_at ? formatRelative(session.last_seen_at) : "—",
    },
    {
      key: "expires",
      header: "Expires",
      render: (session) => formatDateTime(session.expires_at),
    },
    {
      key: "state",
      header: "State",
      render: (session) =>
        session.revoked_at ? (
          <Badge variant="destructive">Revoked</Badge>
        ) : (
          <Badge variant="secondary">Active</Badge>
        ),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Sessions"
        description="Active and revoked refresh-token sessions for your account."
      />
      <Card>
        <CardHeader>
          <CardTitle>Sessions</CardTitle>
          <CardDescription>
            Revoking a session signs out that browser immediately.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={sessions?.length === 0}
            emptyTitle="No sessions"
            emptyDescription="You have no recorded sessions."
          >
            <div className="flex flex-col gap-4">
              <ResourceTable
                columns={columns}
                rows={sessions ?? []}
                rowKey={(session) => session.id}
              />
              <div className="flex justify-end">
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => setTarget(sessions?.find((s) => !s.revoked_at) ?? null)}
                  disabled={!sessions?.some((s) => !s.revoked_at)}
                >
                  Revoke newest active session
                </Button>
              </div>
            </div>
          </AsyncResource>
        </CardContent>
      </Card>

      <ConfirmDialog
        open={Boolean(target)}
        onOpenChange={(open) => !open && setTarget(null)}
        title="Revoke session?"
        description="This signs out that browser. You can sign in again afterward."
        confirmLabel="Revoke"
        destructive
        onConfirm={confirmRevoke}
      />
    </div>
  );
}
