import { useEffect, useState } from "react";
import { Save } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import {
  useUpdateUserSettings,
  useUserSettings,
} from "@/features/personal-settings/api";
import { useI18n } from "@/app/i18n";

export function PersonalSettingsPage() {
  const { t } = useI18n();
  const settings = useUserSettings();
  const update = useUpdateUserSettings();
  const [websocketEnabled, setWebsocketEnabled] = useState(false);

  useEffect(() => {
    if (settings.data) {
      setWebsocketEnabled(settings.data.websocket_enabled);
    }
  }, [settings.data]);

  const save = async () => {
    try {
      await update.mutateAsync({ websocket_enabled: websocketEnabled });
      toast.success(t("Personal settings saved."));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Save failed"));
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Personal settings")}
        description={t("Control optional forwarding capabilities for your account.")}
      />

      <Alert>
        <AlertTitle>{t("Three-layer WebSocket access")}</AlertTitle>
        <AlertDescription>
          {t(
            "Responses WebSocket requests are accepted only when the administrator enables the system, you enable this preference, and the selected upstream channel declares WebSocket support.",
          )}
        </AlertDescription>
      </Alert>

      <AsyncResource isLoading={settings.isLoading} error={settings.error}>
        {settings.data ? (
          <Card>
            <CardHeader>
              <CardTitle>{t("Responses WebSocket")}</CardTitle>
              <CardDescription>
                {t(
                  "Enable your API keys to request the OpenAI Responses WebSocket transport.",
                )}
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-6">
              <Field orientation="horizontal">
                <FieldContent>
                  <FieldLabel htmlFor="personal_websocket_enabled">
                    {t("Enable Responses WebSocket")}
                  </FieldLabel>
                  <FieldDescription>
                    {t(
                      "This preference applies to all active API keys owned by your account.",
                    )}
                  </FieldDescription>
                </FieldContent>
                <Switch
                  id="personal_websocket_enabled"
                  checked={websocketEnabled}
                  onCheckedChange={(checked) => setWebsocketEnabled(Boolean(checked))}
                />
              </Field>

              <Button
                type="button"
                className="self-start"
                disabled={
                  update.isPending ||
                  websocketEnabled === settings.data.websocket_enabled
                }
                onClick={() => void save()}
              >
                {update.isPending ? (
                  <Spinner data-icon="inline-start" />
                ) : (
                  <Save data-icon="inline-start" />
                )}
                {t("Save personal settings")}
              </Button>
            </CardContent>
          </Card>
        ) : null}
      </AsyncResource>
    </div>
  );
}
