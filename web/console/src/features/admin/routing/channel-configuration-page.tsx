import { Navigate, useSearchParams } from "react-router";
import { useI18n } from "@/app/i18n";
import { PageHeader } from "@/components/shared/page-header";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { GroupsPage } from "./groups/groups-page";
import { LogicalChannelsPage } from "./logical-channels/logical-channels-page";

export function ChannelConfigurationPage() {
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  const view = params.get("view") === "groups" ? "groups" : "channels";
  const channelId = params.get("channel");
  if (channelId) return <Navigate replace to={`/admin/routing/logical-channels/${encodeURIComponent(channelId)}?view=capabilities`} />;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={t("Channel configuration")}
        description={t("Filter channels by group. Open a channel to configure its operation capabilities.")} />
      <Tabs value={view} onValueChange={(value) => setParams((current) => {
        const next = new URLSearchParams(current);
        next.set("view", String(value));
        next.delete("page");
        return next;
      })}>
        <TabsList>
          <TabsTrigger value="channels">{t("Logical channels")}</TabsTrigger>
          <TabsTrigger value="groups">{t("Group configuration")}</TabsTrigger>
        </TabsList>
        <TabsContent value="groups"><GroupsPage embedded /></TabsContent>
        <TabsContent value="channels" className="flex flex-col gap-6">
          <LogicalChannelsPage embedded />
        </TabsContent>
      </Tabs>
    </div>
  );
}
