import { useEffect, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, Upload } from "lucide-react";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { discoverPlugins, getPluginJob, installPlugin, listPlugins, pluginKeys } from "./api";
import { PluginAuthorizationDialog, type PluginAction } from "./authorization-dialog";

export function PluginsPage() {
  const { t } = useI18n();
  const client = useQueryClient();
  const [params, setParams] = useSearchParams();
  const [file, setFile] = useState<File | null>(null);
  const [action, setAction] = useState<PluginAction | null>(null);
  const plugins = useQuery({ queryKey: pluginKeys.all, queryFn: ({ signal }) => listPlugins(signal) });
  const jobId = params.get("job");
  const job = useQuery({
    queryKey: ["plugin-job", jobId],
    queryFn: ({ signal }) => getPluginJob(jobId!, signal),
    enabled: Boolean(jobId),
    refetchInterval: (query) => ["queued", "running"].includes(query.state.data?.status ?? "") ? 1000 : false,
  });
  useEffect(() => {
    if (job.data?.status === "succeeded") void client.invalidateQueries({ queryKey: pluginKeys.all });
  }, [client, job.data?.status]);
  const running = job.data?.status === "queued" || job.data?.status === "running";
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Plugins" description="Manage installed native plugins independently from system settings."
        actions={<Button variant="outline" disabled={Boolean(action) || running}
          onClick={() => setAction({ title: "Discover plugins", run: async (token) => {
            const result = await discoverPlugins(token);
            setParams({ job: result.id });
          } })}><RefreshCw data-icon="inline-start" />{t("Discover plugins")}</Button>} />
      <Alert>
        <AlertTitle>{t("Trusted native code only")}</AlertTitle>
        <AlertDescription>{t("Plugins run with gateway privileges and may crash the process. Installing does not enable a plugin. Disabling does not unload its code.")}</AlertDescription>
      </Alert>
      <Card>
        <CardHeader><CardTitle>{t("Install plugin")}</CardTitle>
          <CardDescription>{t("Upload a reviewed tar.gz plugin package. A successful job installs it without granting channel capabilities.")}</CardDescription></CardHeader>
        <CardContent>
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="plugin-package">{t("Plugin package")}</FieldLabel>
              <Input id="plugin-package" type="file" accept=".tar.gz,.tgz,application/gzip"
                disabled={Boolean(action) || running} onChange={(event) => setFile(event.target.files?.[0] ?? null)} />
            </Field>
            <Button className="self-start" disabled={!file || Boolean(action) || running}
              onClick={() => {
                if (file) setAction({ title: "Install plugin", run: async (token) => {
                  const result = await installPlugin(file, token);
                  setParams({ job: result.id });
                } });
              }}><Upload data-icon="inline-start" />{t("Install plugin")}</Button>
          </FieldGroup>
        </CardContent>
      </Card>
      {jobId ? (
        <Card>
          <CardHeader><CardTitle>{t("Plugin operation")}</CardTitle>
            <CardDescription className="break-all">{jobId}</CardDescription></CardHeader>
          <CardContent aria-live="polite">
            <AsyncResource isLoading={job.isPending} error={job.error}>
              {job.data ? <div className="flex flex-col gap-2">
                <Badge variant={job.data.status === "failed" ? "destructive" : "secondary"}>{t(job.data.status)}</Badge>
                {job.data.error_code ? <p>{t("Plugin operation failed ({code}).", { code: job.data.error_code })}</p> : null}
                {job.data.plugin_id ? <Link to={`/admin/plugins/${encodeURIComponent(job.data.plugin_id)}`} className="underline">{job.data.plugin_id}</Link> : null}
              </div> : null}
            </AsyncResource>
            {job.error ? <Button variant="outline" onClick={() => void job.refetch()}>{t("Retry")}</Button> : null}
          </CardContent>
        </Card>
      ) : null}
      <AsyncResource isLoading={plugins.isPending} error={plugins.error}
        isEmpty={plugins.data?.length === 0} emptyTitle="No installed plugins">
        <Card>
          <CardHeader><CardTitle>{t("Installed plugins")}</CardTitle></CardHeader>
          <CardContent>
            <Table>
              <TableHeader><TableRow>
                <TableHead>{t("Plugin")}</TableHead><TableHead>{t("Version")}</TableHead>
                <TableHead>{t("Status")}</TableHead><TableHead>{t("Revision")}</TableHead>
              </TableRow></TableHeader>
              <TableBody>{plugins.data?.map((plugin) => <TableRow key={plugin.id}>
                <TableCell><Link className="underline" to={`/admin/plugins/${encodeURIComponent(plugin.id)}`}>{plugin.id}</Link>
                  {plugin.built_in ? <Badge variant="outline" className="ml-2">{t("Built in")}</Badge> : null}</TableCell>
                <TableCell>{plugin.version ?? "—"}</TableCell>
                <TableCell><Badge variant={plugin.status === "unavailable" ? "destructive" : "secondary"}>{t(plugin.status)}</Badge></TableCell>
                <TableCell>{plugin.revision}</TableCell>
              </TableRow>)}</TableBody>
            </Table>
          </CardContent>
        </Card>
      </AsyncResource>
      {plugins.error ? <Button variant="outline" onClick={() => void plugins.refetch()}>{t("Retry")}</Button> : null}
      {action ? <PluginAuthorizationDialog action={action} onClose={() => setAction(null)} /> : null}
    </div>
  );
}
