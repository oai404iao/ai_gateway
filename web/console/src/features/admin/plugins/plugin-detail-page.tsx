import { useState } from "react";
import { useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/errors";
import type { PluginSettingsValues, PluginView } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { deletePluginArtifact, getPlugin, getPluginSettings, pluginKeys, updatePluginSettings, updatePluginState } from "./api";
import { PluginAuthorizationDialog, type PluginAction } from "./authorization-dialog";
import { PluginSettingsForm } from "./settings-form";

export function PluginDetailPage() {
  const { id = "" } = useParams();
  const { t } = useI18n();
  const client = useQueryClient();
  const [action, setAction] = useState<PluginAction | null>(null);
  const detail = useQuery({ queryKey: pluginKeys.detail(id), queryFn: ({ signal }) => getPlugin(id, signal), refetchOnWindowFocus: false });
  const settings = useQuery({
    queryKey: pluginKeys.settings(id), queryFn: ({ signal }) => getPluginSettings(id, signal),
    enabled: Boolean(detail.data && !detail.data.data.built_in && detail.data.data.artifact_digest),
    refetchOnWindowFocus: false,
  });
  const execute = async (operation: () => Promise<unknown>, success: string) => {
    try {
      await operation();
      await client.invalidateQueries({ queryKey: pluginKeys.all });
      toast.success(t(success));
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        setAction(null);
        toast.error(t("Plugin changed elsewhere. Reloading."));
        await client.invalidateQueries({ queryKey: pluginKeys.all });
      }
      throw error;
    }
  };
  const save = (values: PluginSettingsValues) => {
    if (!settings.data) return;
    const { data, etag } = settings.data;
    setAction({ title: "Save plugin settings", run: (token) => execute(
      () => updatePluginSettings(id, { schema_version: data.schema_version, values }, etag, token),
      "Plugin settings saved and applied.",
    ) });
  };
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={id} description="Plugin versions and settings" backTo="/admin/plugins" backLabel="Plugins" />
      <AsyncResource isLoading={detail.isPending} error={detail.error}>
        {detail.data ? <>
          <PluginVersionCard key={detail.data.etag} plugin={detail.data.data} busy={Boolean(action)}
            onChange={(enabled, digest) => {
              const etag = detail.data!.etag;
              setAction({ title: enabled ? "Enable selected version" : "Disable plugin", run: (token) => execute(
                () => updatePluginState(id, { enabled, artifact_digest: digest }, etag, token), "Plugin state updated.",
              ) });
            }}
            onDelete={(digest) => {
              const etag = detail.data!.etag;
              setAction({ title: "Delete artifact", run: (token) => execute(
                () => deletePluginArtifact(id, digest, etag, token), "Plugin artifact deleted.",
              ) });
            }} />
          {!detail.data.data.built_in && detail.data.data.artifact_digest ? (
            <><AsyncResource isLoading={settings.isPending} error={settings.error}>
              {settings.data ? <PluginSettingsForm key={`${id}:${settings.data.etag}`} settings={settings.data.data}
                saving={Boolean(action)} onSave={save} /> : null}
            </AsyncResource>
            {settings.error ? <Button variant="outline" onClick={() => void settings.refetch()}>{t("Retry")}</Button> : null}</>
          ) : null}
        </> : null}
      </AsyncResource>
      {detail.error ? <Button variant="outline" onClick={() => void detail.refetch()}>{t("Retry")}</Button> : null}
      {action ? <PluginAuthorizationDialog action={action} onClose={() => setAction(null)} /> : null}
    </div>
  );
}

function PluginVersionCard({ plugin, busy, onChange, onDelete }: {
  plugin: PluginView;
  busy: boolean;
  onChange: (enabled: boolean, digest: string | null) => void;
  onDelete: (digest: string) => void;
}) {
  const { t } = useI18n();
  const [digest, setDigest] = useState(plugin.artifact_digest ?? plugin.artifacts[0]?.digest ?? "");
  return (
    <Card>
      <CardHeader><CardTitle>{t("Plugin version")}</CardTitle>
        <CardDescription>{t("Version changes apply to new operations. In-flight operations retain their current generation.")}</CardDescription></CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-wrap gap-2"><Badge>{t(plugin.status)}</Badge>
          <Badge variant="outline">{plugin.version ?? "—"}</Badge>
          <Badge variant="outline">{t("Revision")} {plugin.revision}</Badge></div>
        {plugin.error_code ? <Alert variant="destructive"><AlertTitle>{t("Plugin unavailable")}</AlertTitle>
          <AlertDescription>{plugin.error_code}</AlertDescription></Alert> : null}
        {plugin.built_in ? <p className="text-sm text-muted-foreground">{t("Built-in connector. Lifecycle and plugin settings are not editable.")}</p> : <>
          <FieldGroup>
            <Field><FieldLabel htmlFor="plugin-version">{t("Installed version")}</FieldLabel>
              <Select value={digest} onValueChange={(value) => setDigest(value ?? "")} disabled={busy || plugin.artifacts.length === 0}>
                <SelectTrigger id="plugin-version"><SelectValue>
                  {plugin.artifacts.find((artifact) => artifact.digest === digest)?.version ?? t("Select an option")}
                </SelectValue></SelectTrigger>
                <SelectContent><SelectGroup>{plugin.artifacts.map((artifact) => <SelectItem key={artifact.digest} value={artifact.digest}>
                  {artifact.version} · {artifact.digest.slice(0, 12)}
                </SelectItem>)}</SelectGroup></SelectContent>
              </Select>
            </Field>
          </FieldGroup>
          {digest ? <code className="break-all text-xs">{digest}</code> : null}
          <div className="flex flex-wrap gap-2">
            <Button disabled={busy || !digest || (plugin.enabled && digest === plugin.artifact_digest)}
              onClick={() => onChange(true, digest)}>{t("Enable selected version")}</Button>
            <Button variant="outline" disabled={busy || !plugin.enabled}
              onClick={() => onChange(false, plugin.artifact_digest)}>{t("Disable plugin")}</Button>
            <Button variant="destructive" disabled={busy || !digest || digest === plugin.artifact_digest}
              onClick={() => onDelete(digest)}>{t("Delete artifact")}</Button>
          </div>
        </>}
      </CardContent>
    </Card>
  );
}
