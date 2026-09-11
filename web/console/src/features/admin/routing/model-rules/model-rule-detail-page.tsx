import { ArrowRight, Calculator, Plus } from "lucide-react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { toast } from "sonner";
import { useI18n } from "@/app/i18n";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type { ApiFormat } from "@/api/types";
import { DetailField } from "@/components/shared/detail-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import {
  useCreateModelProtocolRule,
  useModelRule,
} from "@/features/admin/api";
import {
  adminPath,
  safeAdminReturnPath,
} from "@/features/admin/model-setup/model-setup-navigation";
import { API_FORMATS, apiFormatLabel } from "@/lib/permissions";

export function ModelRuleDetailPage() {
  const { id = "" } = useParams();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { t } = useI18n();
  const rule = useModelRule(id);
  const createProtocol = useCreateModelProtocolRule(id);
  const returnTo = safeAdminReturnPath(
    searchParams.get("returnTo"),
    "/admin/routing/model-rules",
  );
  const configuredFormats = new Set(
    rule.data?.data.protocol_rules.map((protocol) => protocol.api_format) ?? [],
  );

  const addProtocol = async (apiFormat: ApiFormat) => {
    try {
      const created = await createProtocol.mutateAsync({
        api_format: apiFormat,
      });
      toast.success(t("Protocol rule created"));
      navigate(
        adminPath(
          `/admin/routing/model-rules/${id}/protocols/${created.id}`,
          { returnTo },
        ),
      );
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This protocol is already configured."));
      } else {
        toast.error(
          controlPlaneMutationErrorMessage(error, t("Create failed")),
        );
      }
    }
  };

  const data = rule.data?.data;
  return (
    <AdminDetailShell
      configurationLens="routes"
      title={data?.model_display_name ?? t("Model Rules")}
      description={t(
        "One priced client model with independent protocol routing rules.",
      )}
      backPath={returnTo}
      backLabel={t("Back to rules")}
      isLoading={rule.isLoading}
      error={rule.error}
      hasData={Boolean(data)}
      detailCard={
        data ? (
          <Card size="sm">
            <CardHeader>
              <CardTitle>{data.model_display_name}</CardTitle>
              <CardDescription className="font-mono">
                {data.client_model}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <dl className="flex flex-col gap-4">
                <DetailField
                  label={t("Pricing model")}
                  value={data.client_model}
                  mono
                />
                <DetailField
                  label={t("Provider")}
                  value={data.model_provider_name ?? t("Unspecified provider")}
                />
                <DetailField
                  label={t("Model enabled")}
                  value={<StatusBadge value={data.model_enabled} />}
                />
              </dl>
              <Button
                className="mt-4"
                nativeButton={false}
                role="link"
                variant="outline"
                size="sm"
                render={
                  <Link
                    to={adminPath(`/admin/models/${data.model_id}/pricing`, {
                      returnTo: adminPath(
                        `/admin/routing/model-rules/${data.id}`,
                        { returnTo },
                      ),
                    })}
                  />
                }
              >
                <Calculator data-icon="inline-start" />
                {t("Configure pricing")}
              </Button>
            </CardContent>
          </Card>
        ) : null
      }
      editCard={
        data ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Protocol rules")}</CardTitle>
              <CardDescription>
                {t(
                  "Add only the client protocols this model supports. Each protocol owns its priority tiers and upstream model mappings.",
                )}
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              {API_FORMATS.map((apiFormat) => {
                const protocol = data.protocol_rules.find(
                  (candidate) => candidate.api_format === apiFormat,
                );
                if (!protocol) {
                  return (
                    <Card key={apiFormat} size="sm">
                      <CardHeader>
                        <CardTitle>{apiFormatLabel(apiFormat)}</CardTitle>
                        <CardDescription>{t("Not configured")}</CardDescription>
                        <CardAction>
                          <Button
                            size="sm"
                            variant="outline"
                            disabled={
                              createProtocol.isPending ||
                              configuredFormats.has(apiFormat)
                            }
                            onClick={() => void addProtocol(apiFormat)}
                          >
                            {createProtocol.isPending ? (
                              <Spinner data-icon="inline-start" />
                            ) : (
                              <Plus data-icon="inline-start" />
                            )}
                            {t("Add protocol")}
                          </Button>
                        </CardAction>
                      </CardHeader>
                    </Card>
                  );
                }
                return (
                  <Card key={protocol.id} size="sm">
                    <CardHeader>
                      <CardTitle>{apiFormatLabel(protocol.api_format)}</CardTitle>
                      <CardDescription>
                        {t(
                          "{tiers} priority tiers · {active}/{targets} active targets",
                          {
                            tiers: protocol.routing_tiers.length,
                            active: protocol.active_channel_count,
                            targets: protocol.target_channel_count,
                          },
                        )}
                      </CardDescription>
                      <CardAction className="flex items-center gap-2">
                        <StatusBadge value={protocol.routing_status} />
                        <Button
                          nativeButton={false}
                          role="link"
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("Open {protocol}", {
                            protocol: apiFormatLabel(protocol.api_format),
                          })}
                          render={
                            <Link
                              to={adminPath(
                                `/admin/routing/model-rules/${id}/protocols/${protocol.id}`,
                                { returnTo },
                              )}
                            />
                          }
                        >
                          <ArrowRight />
                        </Button>
                      </CardAction>
                    </CardHeader>
                    <CardContent className="flex flex-wrap gap-2">
                      <Badge variant="secondary">
                        {t("{count} tiers", {
                          count: protocol.routing_tiers.length,
                        })}
                      </Badge>
                      <Badge variant="outline">
                        {t("{count} model-capable channels", {
                          count: protocol.model_capable_channel_count,
                        })}
                      </Badge>
                    </CardContent>
                  </Card>
                );
              })}
            </CardContent>
          </Card>
        ) : null
      }
    />
  );
}
