import { ArrowRight, Boxes, Calculator, Route } from "lucide-react";
import { useNavigate } from "react-router";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  useLogicalChannels,
  useOperationRules,
  useModels,
} from "@/features/admin/api";
import { ConfigurationNav } from "./configuration-navigation";

export function ModelSetupPage() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const rules = useOperationRules();
  const models = useModels();
  const channels = useLogicalChannels();
  const loading =
    rules.isLoading ||
    models.isLoading ||
    channels.isLoading;
  const error = rules.error ?? models.error ?? channels.error;
  const protocolCount = rules.data?.length ?? 0;

  return (
    <div className="space-y-6">
      <PageHeader
        title={t("Model setup")}
        description={t(
          "Configure pricing models, protocol-aware routing rules, and channel supply in their dedicated views.",
        )}
      />
      <ConfigurationNav />
      <AsyncResource isLoading={loading} error={error}>
        <div className="grid gap-4 lg:grid-cols-3">
          <SetupCard
            title={t("1. Pricing models")}
            description={t(
              "Define the client-visible model identity and its billing prices.",
            )}
            count={models.data?.length ?? 0}
            countLabel={t("models")}
            action={t("Manage pricing")}
            icon={Calculator}
            onClick={() => navigate("/admin/models")}
          />
          <SetupCard
            title={t("2. Channels")}
            description={t(
              "Advertise the upstream model identifiers each channel supports.",
            )}
            count={channels.data?.length ?? 0}
            countLabel={t("channels")}
            action={t("Manage channels")}
            icon={Boxes}
            onClick={() => navigate("/admin/routing/logical-channels")}
          />
          <SetupCard
            title={t("3. Model rules")}
            description={t(
              "Attach protocols to priced models, then configure priority tiers and explicit channel/model candidates.",
            )}
            count={protocolCount}
            countLabel={t("Operation rules")}
            action={t("Manage routing")}
            icon={Route}
            onClick={() => navigate("/admin/routing/operation-rules")}
          />
        </div>
      </AsyncResource>
    </div>
  );
}

function SetupCard({
  title,
  description,
  count,
  countLabel,
  action,
  icon: Icon,
  onClick,
}: {
  title: string;
  description: string;
  count: number;
  countLabel: string;
  action: string;
  icon: typeof Route;
  onClick: () => void;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Icon className="size-4" />
          {title}
        </CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        <Badge variant="secondary">
          {count} {countLabel}
        </Badge>
      </CardContent>
      <CardFooter>
        <Button variant="outline" onClick={onClick}>
          {action}
          <ArrowRight data-icon="inline-end" />
        </Button>
      </CardFooter>
    </Card>
  );
}
