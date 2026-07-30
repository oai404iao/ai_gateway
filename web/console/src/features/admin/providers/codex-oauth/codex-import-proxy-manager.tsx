import { useEffect, useState } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type {
  ProxyCreateInput,
  ProxyInput,
  ProxyView,
} from "@/api/types";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import { useI18n } from "@/app/i18n";
import { StatusBadge } from "@/components/shared/status-badge";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  useCreateProxy,
  useDeleteProxy,
  useProxy,
  useUpdateProxy,
} from "@/features/admin/api";
import type { ImportedProxyDraft } from "./codex-import";

const NO_MATCH = "__unmatched__";

interface ProxyFormState {
  name: string;
  proxy_url: string;
  username: string;
  password: string;
  no_proxy_hosts: string;
  enabled: boolean;
}

interface ProxyDialogTarget {
  mode: "create" | "edit";
  proxyId: string;
  sourceKey: string;
  initial: ProxyFormState;
}

const EMPTY_PROXY: ProxyFormState = {
  name: "",
  proxy_url: "",
  username: "",
  password: "",
  no_proxy_hosts: "",
  enabled: true,
};

export function CodexImportProxyManager({
  proxies,
  importedProxies,
  onAssignImportedProxy,
  onProxyDeleted,
}: {
  proxies: ProxyView[];
  importedProxies: ImportedProxyDraft[];
  onAssignImportedProxy: (sourceKey: string, proxyId: string) => void;
  onProxyDeleted: (proxyId: string) => void;
}) {
  const { t } = useI18n();
  const [dialogTarget, setDialogTarget] = useState<ProxyDialogTarget | null>(
    null,
  );
  const [deleteTarget, setDeleteTarget] = useState<ProxyView | null>(null);
  const deleteDetail = useProxy(deleteTarget?.id ?? "");
  const deleteProxy = useDeleteProxy(deleteTarget?.id ?? "");

  const openImportedProxy = (proxy: ImportedProxyDraft) => {
    setDialogTarget({
      mode: "create",
      proxyId: "",
      sourceKey: proxy.source_key,
      initial: {
        name: proxy.name,
        proxy_url: proxy.proxy_url,
        username: proxy.username ?? "",
        password: proxy.password ?? "",
        no_proxy_hosts: proxy.no_proxy_hosts.join("\n"),
        enabled: proxy.enabled,
      },
    });
  };

  const removeProxy = async () => {
    if (!deleteTarget || !deleteDetail.etag) return;
    try {
      await deleteProxy.mutateAsync({ ifMatch: deleteDetail.etag });
      toast.success(t("Proxy deleted"));
      onProxyDeleted(deleteTarget.id);
      setDeleteTarget(null);
    } catch (error) {
      if (error instanceof ApiError && error.code === "proxy_in_use") {
        toast.error(t("This proxy is still assigned and cannot be deleted."));
      } else if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This proxy was changed elsewhere. Reload and try again."));
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Delete failed")));
      }
    }
  };

  return (
    <Card>
      <CardHeader>
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex flex-col gap-1">
            <CardTitle>{t("Proxy configuration")}</CardTitle>
            <CardDescription>
              {t(
                "Create, edit, delete, and map egress proxies without leaving the import review.",
              )}
            </CardDescription>
          </div>
          <Button
            variant="outline"
            onClick={() =>
              setDialogTarget({
                mode: "create",
                proxyId: "",
                sourceKey: "",
                initial: EMPTY_PROXY,
              })
            }
          >
            <Plus data-icon="inline-start" />
            {t("New proxy")}
          </Button>
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-6">
        {importedProxies.length > 0 ? (
          <div className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <h3 className="font-medium">{t("Proxies found in import")}</h3>
              <p className="text-sm text-muted-foreground">
                {t(
                  "Review each source proxy, create it, or map it to an existing proxy before importing credentials.",
                )}
              </p>
            </div>
            <div className="overflow-x-auto rounded-xl border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t("Imported proxy")}</TableHead>
                    <TableHead>{t("Endpoint")}</TableHead>
                    <TableHead>{t("Map to existing")}</TableHead>
                    <TableHead className="text-right">{t("Actions")}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {importedProxies.map((proxy) => (
                    <TableRow key={proxy.id}>
                      <TableCell className="min-w-52">
                        <div className="font-medium">{proxy.name}</div>
                        <div className="mt-1 flex flex-wrap gap-1">
                          <Badge variant="outline">{proxy.source}</Badge>
                          {proxy.username || proxy.password ? (
                            <Badge variant="secondary">
                              {t("Credential included")}
                            </Badge>
                          ) : null}
                        </div>
                        {proxy.errors.map((error) => (
                          <p
                            className="mt-1 text-xs text-destructive"
                            key={error}
                          >
                            {t(error)}
                          </p>
                        ))}
                      </TableCell>
                      <TableCell className="min-w-64 font-mono text-xs">
                        {proxy.proxy_url || "—"}
                      </TableCell>
                      <TableCell className="min-w-56">
                        <Select
                          value={proxy.existing_proxy_id || NO_MATCH}
                          onValueChange={(value) =>
                            onAssignImportedProxy(
                              proxy.source_key,
                              value === NO_MATCH ? "" : value,
                            )
                          }
                        >
                          <SelectTrigger
                            aria-label={t("Map {name} to existing proxy", {
                              name: proxy.name,
                            })}
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectGroup>
                              <SelectItem value={NO_MATCH}>
                                {t("Not mapped")}
                              </SelectItem>
                              {proxies.map((existing) => (
                                <SelectItem
                                  key={existing.id}
                                  value={existing.id}
                                >
                                  {existing.name}
                                  {!existing.enabled
                                    ? ` · ${t("Disabled")}`
                                    : ""}
                                </SelectItem>
                              ))}
                            </SelectGroup>
                          </SelectContent>
                        </Select>
                      </TableCell>
                      <TableCell className="text-right">
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => openImportedProxy(proxy)}
                          disabled={proxy.errors.length > 0}
                        >
                          <Plus data-icon="inline-start" />
                          {t("Review and create")}
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </div>
        ) : null}

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <h3 className="font-medium">{t("Existing proxies")}</h3>
            <p className="text-sm text-muted-foreground">
              {t("Only enabled proxies can be assigned to a Codex credential.")}
            </p>
          </div>
          <div className="overflow-x-auto rounded-xl border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("Name")}</TableHead>
                  <TableHead>{t("Proxy URL")}</TableHead>
                  <TableHead>{t("Status")}</TableHead>
                  <TableHead className="text-right">{t("Actions")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {proxies.length === 0 ? (
                  <TableRow>
                    <TableCell
                      colSpan={4}
                      className="text-center text-muted-foreground"
                    >
                      {t("No proxies configured")}
                    </TableCell>
                  </TableRow>
                ) : (
                  proxies.map((proxy) => (
                    <TableRow key={proxy.id}>
                      <TableCell className="font-medium">{proxy.name}</TableCell>
                      <TableCell className="font-mono text-xs">
                        {proxy.proxy_url}
                      </TableCell>
                      <TableCell>
                        <StatusBadge value={proxy.enabled} />
                      </TableCell>
                      <TableCell>
                        <div className="flex justify-end gap-1">
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("Edit {name}", { name: proxy.name })}
                            onClick={() =>
                              setDialogTarget({
                                mode: "edit",
                                proxyId: proxy.id,
                                sourceKey: "",
                                initial: EMPTY_PROXY,
                              })
                            }
                          >
                            <Pencil />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("Delete {name}", {
                              name: proxy.name,
                            })}
                            onClick={() => setDeleteTarget(proxy)}
                          >
                            <Trash2 />
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        </div>
      </CardContent>

      <ProxyEditorDialog
        target={dialogTarget}
        onClose={() => setDialogTarget(null)}
        onCreated={(sourceKey, proxyId) => {
          if (sourceKey) onAssignImportedProxy(sourceKey, proxyId);
        }}
      />

      <AlertDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("Delete proxy?")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                "The proxy can be deleted only when no channel or pending Codex authorization uses it.",
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("Cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => void removeProxy()}
              disabled={deleteDetail.isLoading || deleteProxy.isPending}
            >
              {deleteProxy.isPending ? (
                <Spinner data-icon="inline-start" />
              ) : null}
              {t("Delete proxy")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}

function ProxyEditorDialog({
  target,
  onClose,
  onCreated,
}: {
  target: ProxyDialogTarget | null;
  onClose: () => void;
  onCreated: (sourceKey: string, proxyId: string) => void;
}) {
  const { t } = useI18n();
  const proxyId = target?.mode === "edit" ? target.proxyId : "";
  const detail = useProxy(proxyId);
  const create = useCreateProxy();
  const update = useUpdateProxy(proxyId);
  const [state, setState] = useState<ProxyFormState>(EMPTY_PROXY);
  const [errors, setErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!target) return;
    if (target.mode === "create") {
      setState(target.initial);
      setErrors({});
      return;
    }
    if (detail.data) {
      setState({
        name: detail.data.data.name,
        proxy_url: detail.data.data.proxy_url,
        username: "",
        password: "",
        no_proxy_hosts: detail.data.data.no_proxy_hosts.join("\n"),
        enabled: detail.data.data.enabled,
      });
      setErrors({});
    }
  }, [detail.data, target]);

  const submit = async () => {
    const validation = validateProxy(state);
    setErrors(validation);
    if (Object.keys(validation).length > 0 || !target) return;
    const noProxyHosts = state.no_proxy_hosts
      .split(/[\n,]/)
      .map((value) => value.trim())
      .filter(Boolean);
    try {
      if (target.mode === "create") {
        const input: ProxyCreateInput = {
          name: state.name.trim(),
          proxy_url: state.proxy_url.trim(),
          username: state.username.trim() || null,
          password: state.password || null,
          no_proxy_hosts: noProxyHosts,
          enabled: state.enabled,
        };
        const result = await create.mutateAsync(input);
        onCreated(target.sourceKey, result.id);
        toast.success(t("Proxy created"));
      } else {
        if (!detail.etag) return;
        const input: ProxyInput = {
          name: state.name.trim(),
          proxy_url: state.proxy_url.trim(),
          no_proxy_hosts: noProxyHosts,
          enabled: state.enabled,
        };
        if (state.username) input.username = state.username;
        if (state.password) input.password = state.password;
        await update.mutateAsync({ input, ifMatch: detail.etag });
        toast.success(t("Proxy updated"));
      }
      onClose();
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This proxy was changed elsewhere. Reload and try again."));
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
      }
    }
  };

  const patch = (partial: Partial<ProxyFormState>) => {
    setState((current) => ({ ...current, ...partial }));
    setErrors({});
  };
  const pending = create.isPending || update.isPending;

  return (
    <Dialog open={Boolean(target)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>
            {target?.mode === "edit" ? t("Edit proxy") : t("Create proxy")}
          </DialogTitle>
          <DialogDescription>
            {target?.mode === "edit"
              ? t("Leave credential fields blank to keep current values.")
              : t("Review imported proxy details before saving.")}
          </DialogDescription>
        </DialogHeader>
        {target?.mode === "edit" && detail.isLoading ? (
          <div className="flex min-h-32 items-center justify-center">
            <Spinner />
          </div>
        ) : (
          <FieldGroup className="grid gap-5 sm:grid-cols-2">
            <Field data-invalid={Boolean(errors.name)}>
              <FieldLabel htmlFor="import-proxy-name">{t("Name")}</FieldLabel>
              <Input
                id="import-proxy-name"
                value={state.name}
                onChange={(event) => patch({ name: event.target.value })}
                aria-invalid={Boolean(errors.name)}
              />
              {errors.name ? <FieldError>{t(errors.name)}</FieldError> : null}
            </Field>
            <Field data-invalid={Boolean(errors.proxy_url)}>
              <FieldLabel htmlFor="import-proxy-url">
                {t("Proxy URL")}
              </FieldLabel>
              <Input
                id="import-proxy-url"
                value={state.proxy_url}
                onChange={(event) => patch({ proxy_url: event.target.value })}
                placeholder="socks5h://proxy.example:1080"
                aria-invalid={Boolean(errors.proxy_url)}
              />
              {errors.proxy_url ? (
                <FieldError>{t(errors.proxy_url)}</FieldError>
              ) : null}
            </Field>
            <Field>
              <FieldLabel htmlFor="import-proxy-username">
                {t("Username")}
              </FieldLabel>
              <Input
                id="import-proxy-username"
                value={state.username}
                onChange={(event) => patch({ username: event.target.value })}
                autoComplete="off"
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="import-proxy-password">
                {t("Password")}
              </FieldLabel>
              <Input
                id="import-proxy-password"
                type="password"
                value={state.password}
                onChange={(event) => patch({ password: event.target.value })}
                autoComplete="new-password"
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="import-proxy-no-proxy">
                {t("No-proxy hosts")}
              </FieldLabel>
              <Input
                id="import-proxy-no-proxy"
                value={state.no_proxy_hosts}
                onChange={(event) =>
                  patch({ no_proxy_hosts: event.target.value })
                }
                placeholder="example.com, .internal"
              />
              <FieldDescription>
                {t("Separate hosts with commas or new lines.")}
              </FieldDescription>
            </Field>
            <Field orientation="horizontal">
              <FieldLabel htmlFor="import-proxy-enabled">
                {t("Enabled")}
              </FieldLabel>
              <Switch
                id="import-proxy-enabled"
                checked={state.enabled}
                onCheckedChange={(enabled) =>
                  patch({ enabled: Boolean(enabled) })
                }
              />
            </Field>
          </FieldGroup>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("Cancel")}
          </Button>
          <Button
            onClick={() => void submit()}
            disabled={pending || detail.isLoading}
          >
            {pending ? <Spinner data-icon="inline-start" /> : null}
            {target?.mode === "edit" ? t("Save proxy") : t("Create proxy")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function validateProxy(state: ProxyFormState): Record<string, string> {
  const errors: Record<string, string> = {};
  if (!state.name.trim()) errors.name = "Name is required.";
  try {
    const url = new URL(state.proxy_url.trim());
    if (
      ![
        "http:",
        "https:",
        "socks4:",
        "socks4a:",
        "socks5:",
        "socks5h:",
      ].includes(url.protocol) ||
      !url.hostname ||
      url.username ||
      url.password ||
      (url.pathname !== "" && url.pathname !== "/") ||
      url.search ||
      url.hash
    ) {
      throw new Error("invalid proxy");
    }
  } catch {
    errors.proxy_url =
      "Enter an HTTP(S) or SOCKS proxy URL without embedded credentials, path, query, or fragment.";
  }
  return errors;
}
