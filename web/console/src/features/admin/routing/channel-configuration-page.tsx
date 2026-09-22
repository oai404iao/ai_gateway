import { useSearchParams } from "react-router";
import { useI18n } from "@/app/i18n";
import { PageHeader } from "@/components/shared/page-header";
import { EmptyState } from "@/components/shared/empty-state";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { GroupsPage } from "./groups/groups-page";
import { LogicalChannelsPage } from "./logical-channels/logical-channels-page";
import { CapabilitiesPage } from "./capabilities/capabilities-page";

export function ChannelConfigurationPage() {
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  const view = params.get("view") === "groups" ? "groups" : "channels";
  const channelId = params.get("channel");

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={t("Channel configuration")}
        description={t("Manage groups, their logical channels, and each channel's operation capabilities.")} />
      <Tabs value={view} onValueChange={(value) => setParams((current) => {
        const next = new URLSearchParams(current);
        next.set("view", String(value));
        return next;
      })}>
        <TabsList>
          <TabsTrigger value="channels">{t("Logical channels")}</TabsTrigger>
          <TabsTrigger value="groups">{t("Group configuration")}</TabsTrigger>
        </TabsList>
        <TabsContent value="groups"><GroupsPage /></TabsContent>
        <TabsContent value="channels" className="flex flex-col gap-6">
          <LogicalChannelsPage configureCapabilities />
          {channelId
            ? <CapabilitiesPage key={channelId} channelId={channelId} />
            : <EmptyState title={t("Select a channel to configure its capabilities")} />}
        </TabsContent>
      </Tabs>
    </div>
  );
}
