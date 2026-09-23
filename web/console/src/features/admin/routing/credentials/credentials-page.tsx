import { lazy, Suspense } from "react";
import { useSearchParams } from "react-router";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { useUpstreamCredentials } from "@/features/admin/api";
import { StatusBadge } from "@/components/shared/status-badge";
import { useI18n } from "@/app/i18n";
import { PageHeader } from "@/components/shared/page-header";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";

const CodexCredentials = lazy(() => import("@/features/admin/providers/codex-oauth/codex-oauth-page"));

export function CredentialsPage() {
  const [params, setParams] = useSearchParams();
  const query = useUpstreamCredentials();
  const { t } = useI18n();
  const connector = params.get("connector") === "codex" ? "codex" : "general";
  return <div className="flex min-w-0 flex-col gap-6">
    <PageHeader title={t("Upstream credentials")}
      description={t("Reusable upstream identities. Rotating a credential updates every referencing channel.")} />
    <Tabs value={connector} onValueChange={(value) => setParams((current) => {
      const next = new URLSearchParams(current);
      next.set("connector", String(value));
      next.delete("page");
      return next;
    })} className="min-w-0">
      <TabsList aria-label={t("Connector")}>
        <TabsTrigger value="general">{t("General")}</TabsTrigger>
        <TabsTrigger value="codex">Codex</TabsTrigger>
      </TabsList>
      <TabsContent value="general" className="min-w-0">
        <AdminListPage
    embedded
    title={t("General credentials")}
    description={t("Bearer tokens and custom authentication headers for compatible upstream accesses.")}
    query={{ ...query, data: query.data?.filter((credential) => credential.connector_kind === "general") }}
    rowKey={(credential) => credential.id}
    detailPath={(credential) => `/admin/routing/upstream-credentials/${credential.id}`}
    createLabel={t("New credential")}
    createPath="/admin/routing/upstream-credentials/new"
    columns={[
      { key: "name", header: t("Name"), render: (credential) => credential.name },
      { key: "kind", header: t("Authentication type"), render: (credential) => credential.kind === "bearer" ? "Bearer" : t("Custom header") },
      { key: "channels", header: t("Channels"), render: (credential) => credential.channel_ids.length },
      { key: "enabled", header: t("Enabled"), render: (credential) => <StatusBadge value={credential.enabled} /> },
    ]}
  />
      </TabsContent>
      <TabsContent value="codex" className="min-w-0">
        <Suspense fallback={<Skeleton className="h-40 w-full" />}><CodexCredentials /></Suspense>
      </TabsContent>
    </Tabs>
  </div>;
}
