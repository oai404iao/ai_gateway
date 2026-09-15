import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { Plus } from "lucide-react";
import { toast } from "sonner";
import { useI18n } from "@/app/i18n";
import { controlPlaneMutationErrorMessage } from "@/api/errors";
import { StatusBadge } from "@/components/shared/status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import {
  useCreateModelRule,
  useModelRules,
  useModels,
} from "@/features/admin/api";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import { ConfigurationTableView } from "@/features/admin/model-setup/configuration-navigation";
import { groupModelsByProvider } from "@/features/admin/models/model-groups";
import { formatRelative } from "@/lib/dates";
import { apiFormatLabel } from "@/lib/permissions";

const NO_MODEL = "__no_model__";

export function ModelRulesPage() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const rules = useModelRules();
  const models = useModels();
  const create = useCreateModelRule();
  const [open, setOpen] = useState(false);
  const [modelId, setModelId] = useState("");
  const usedModelIds = new Set((rules.data ?? []).map((rule) => rule.model_id));
  const availableModels = (models.data ?? []).filter(
    (model) => model.enabled && !usedModelIds.has(model.id),
  );
  const modelGroups = useMemo(
    () =>
      groupModelsByProvider(
        availableModels,
        t("Unspecified provider"),
      ),
    [availableModels, t],
  );

  const createRule = async () => {
    if (!modelId) return;
    try {
      const result = await create.mutateAsync({ model_id: modelId });
      setOpen(false);
      setModelId("");
      toast.success(t("Model rule created"));
      navigate(`/admin/routing/model-rules/${result.id}`);
    } catch (error) {
      toast.error(controlPlaneMutationErrorMessage(error, t("Create failed")));
    }
  };

  return (
    <>
      <ConfigurationTableView lens="routes">
        <AdminListPage
          title={t("Model Rules")}
          description={t(
            "Attach a priced client model once, then configure its protocols and explicit channel/model candidates.",
          )}
          query={{
            data: rules.data,
            isLoading: rules.isLoading || models.isLoading,
            error: rules.error ?? models.error,
          }}
          rowKey={(rule) => rule.id}
          detailPath={(rule) => `/admin/routing/model-rules/${rule.id}`}
          createLabel={t("New model rule")}
          onCreate={() => setOpen(true)}
          columns={[
            {
              key: "model",
              header: t("Client model"),
              render: (rule) => (
                <span className="flex flex-col gap-1">
                  <span className="font-medium">{rule.model_display_name}</span>
                  <span className="font-mono text-xs text-muted-foreground">
                    {rule.client_model}
                  </span>
                </span>
              ),
            },
            {
              key: "protocols",
              header: t("Supported protocols"),
              render: (rule) =>
                rule.protocol_rules.length > 0 ? (
                  <span className="flex flex-wrap gap-1">
                    {rule.protocol_rules.map((protocol) => (
                      <span
                        key={protocol.id}
                        className="flex items-center gap-1"
                      >
                        <Badge variant="outline">
                          {apiFormatLabel(protocol.api_format)}
                        </Badge>
                        <StatusBadge value={protocol.routing_status} />
                      </span>
                    ))}
                  </span>
                ) : (
                  <Badge variant="secondary">{t("No protocols")}</Badge>
                ),
            },
            {
              key: "model_status",
              header: t("Pricing model"),
              render: (rule) => <StatusBadge value={rule.model_enabled} />,
            },
            {
              key: "updated",
              header: t("Updated"),
              render: (rule) => formatRelative(rule.updated_at),
            },
          ]}
        />
      </ConfigurationTableView>

      <Dialog
        open={open}
        onOpenChange={(nextOpen) => {
          setOpen(nextOpen);
          if (!nextOpen) setModelId("");
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("New model rule")}</DialogTitle>
            <DialogDescription>
              {t(
                "Choose an enabled priced model that does not already have a model rule. Protocols can be added afterwards.",
              )}
            </DialogDescription>
          </DialogHeader>
          <Field>
            <FieldLabel>{t("Priced client model")}</FieldLabel>
            <Select
              value={modelId || NO_MODEL}
              onValueChange={(value) =>
                setModelId(value === NO_MODEL ? "" : value)
              }
            >
              <SelectTrigger aria-label={t("Priced client model")}>
                <SelectValue placeholder={t("Choose a priced model")} />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem value={NO_MODEL}>
                    {t("Choose a priced model")}
                  </SelectItem>
                </SelectGroup>
                {modelGroups.map((group) => (
                  <SelectGroup key={group.provider}>
                    <SelectLabel>{group.provider}</SelectLabel>
                    {group.models.map((model) => (
                      <SelectItem key={model.id} value={model.id}>
                        {model.display_name} ({model.source_model_id})
                      </SelectItem>
                    ))}
                  </SelectGroup>
                ))}
              </SelectContent>
            </Select>
            <FieldDescription>
              {availableModels.length > 0
                ? t("{count} models available", {
                    count: availableModels.length,
                  })
                : t("Every enabled priced model already has a rule.")}
            </FieldDescription>
          </Field>
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button
              disabled={!modelId || create.isPending}
              onClick={() => void createRule()}
            >
              {create.isPending ? (
                <Spinner data-icon="inline-start" />
              ) : (
                <Plus data-icon="inline-start" />
              )}
              {t("Create rule")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
