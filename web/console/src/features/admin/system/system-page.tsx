import { useState } from "react";
import { toast } from "sonner";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";
import { PageHeader } from "@/components/shared/page-header";
import { DetailField } from "@/components/shared/detail-field";
import { useReload } from "@/features/admin/api";

export function SystemPage() {
  const reload = useReload();
  const [correlation, setCorrelation] = useState<string | null>(null);

  const run = async () => {
    try {
      const result = await reload.mutateAsync();
      setCorrelation(result.correlation_id);
      toast.success("Control plane reloaded");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Reload failed");
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="System"
        description="Manually refresh the runtime snapshot from PostgreSQL."
      />
      <Card>
        <CardHeader>
          <CardTitle>Reload control plane</CardTitle>
          <CardDescription>
            Re-compiles and publishes the immutable runtime snapshot. Periodic
            reloads also run automatically.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex flex-col gap-4">
            <Button className="self-start" onClick={run} disabled={reload.isPending}>
              {reload.isPending ? <Spinner data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}
              Reload now
            </Button>
            {correlation ? (
              <dl>
                <DetailField label="Correlation id" value={correlation} mono />
              </dl>
            ) : null}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
