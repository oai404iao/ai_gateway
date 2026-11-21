import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Field, FieldContent, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { AsyncResource } from "@/components/shared/async-resource";
import { AccountPage } from "@/features/profile/account-page";
import { useUpdateUserSettings, useUserSettings } from "@/features/personal-settings/api";
import { useI18n } from "@/app/i18n";
import { useConfigurationDraft } from "@/features/admin/model-setup/use-configuration-draft";

export function PersonalSettingsPage() {
  const { t } = useI18n();
  const settings = useUserSettings();
  const update = useUpdateUserSettings();
  const [websocketEnabled, setWebsocketEnabled] = useState(false);
  const draft = useConfigurationDraft(
    update.isPending,
    Boolean(settings.data && websocketEnabled !== settings.data.websocket_enabled),
  );

  useEffect(() => {
    if (settings.data) {
      setWebsocketEnabled(settings.data.websocket_enabled);
    }
  }, [settings.data]);

  const save = async () => {
    try {
      await update.mutateAsync({ websocket_enabled: websocketEnabled });
      draft.markSaved();
      toast.success(t("Personal settings saved."));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("Save failed"));
    }
  };

  return (
    <AccountPage>
      {draft.navigationGuard}
      <AsyncResource isLoading={settings.isLoading} error={settings.error}>
        {settings.data ? (
          <section className="flex flex-col gap-4">
            <h2 className="text-lg font-semibold">{t("Transports")}</h2>
            <Field orientation="horizontal" className="border-b pb-4">
              <FieldContent>
                <FieldLabel htmlFor="personal_websocket_enabled">
                  {t("Enable Responses WebSocket")}
                </FieldLabel>
                <FieldDescription>
                  {t("This preference applies to all active API keys owned by your account.")}
                </FieldDescription>
              </FieldContent>
              <Switch
                id="personal_websocket_enabled"
                checked={websocketEnabled}
                disabled={update.isPending}
                onCheckedChange={(checked) => setWebsocketEnabled(Boolean(checked))}
              />
            </Field>

            <Button
              type="button"
              className="self-end"
              disabled={update.isPending || websocketEnabled === settings.data.websocket_enabled}
              onClick={() => void save()}
            >
              {update.isPending && <Spinner data-icon="inline-start" />}
              {t("Save personal settings")}
            </Button>
          </section>
        ) : null}
      </AsyncResource>
      <section>
        <h2 className="text-lg font-semibold">{t("Connection requirements")}</h2>
        <dl className="mt-3 grid gap-2 text-sm [&_dd]:text-muted-foreground [&_dt]:font-medium">
          <div className="grid grid-cols-[6rem_1fr] gap-3">
            <dt>{t("System")}</dt>
            <dd>{t("The gateway administrator enables WebSocket access.")}</dd>
          </div>
          <div className="grid grid-cols-[6rem_1fr] gap-3">
            <dt>{t("Account")}</dt>
            <dd>{t("You enable and save this preference.")}</dd>
          </div>
          <div className="grid grid-cols-[6rem_1fr] gap-3">
            <dt>{t("Channel")}</dt>
            <dd>{t("The selected upstream channel supports WebSocket.")}</dd>
          </div>
        </dl>
      </section>
    </AccountPage>
  );
}
