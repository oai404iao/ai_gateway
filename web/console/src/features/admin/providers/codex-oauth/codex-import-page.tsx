import { useMemo, useState } from "react";
import {
  FileJson,
  Pencil,
  RotateCcw,
  Trash2,
  Upload,
  WandSparkles,
} from "lucide-react";
import { useReturnPath } from "@/lib/page-navigation";
import { useConfigurationDraft } from "@/hooks/use-configuration-draft";
import { toast } from "sonner";
import type { CodexCredentialImportInput } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { ErrorAlert } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldDescription,
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
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useProxies } from "@/features/admin/api";
import {
  type CodexCredentialImportDraft,
  type CodexImportDocument,
  type ImportedProxyDraft,
  parseCodexImportDocuments,
} from "./codex-import";
import { CodexImportProxyManager } from "./codex-import-proxy-manager";
import { CodexAccountIdentity } from "./codex-account-identity";
import { useImportCodexCredential } from "./api";

import {
  codexImportError,
  CredentialEditorDialog,
  credentialErrors,
  credentialWarnings,
  ImportStatusBadge,
  matchExistingProxy,
  MAX_FILE_BYTES,
  MAX_FILES,
  NO_PROXY,
  SummaryCard,
} from "./codex-import-dialog";

export default function CodexImportPage() {
  const { t } = useI18n();
  const returnTo = useReturnPath("/admin/routing/upstream-credentials?connector=codex");
  const proxiesQuery = useProxies();
  const importCredential = useImportCodexCredential();
  const [jsonText, setJsonText] = useState("");
  const [uploadedDocuments, setUploadedDocuments] = useState<
    CodexImportDocument[]
  >([]);
  const [credentials, setCredentials] = useState<
    CodexCredentialImportDraft[]
  >([]);
  const [importedProxies, setImportedProxies] = useState<
    ImportedProxyDraft[]
  >([]);
  const [parseErrors, setParseErrors] = useState<string[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [bulkProxyId, setBulkProxyId] = useState(NO_PROXY);
  const [importing, setImporting] = useState(false);
  const [sourceDirty, setSourceDirty] = useState(false);
  const { navigationGuard } = useConfigurationDraft(
    importing, sourceDirty || credentials.some((credential) => credential.status !== "imported"),
  );

  const proxies = proxiesQuery.data ?? [];
  const enabledProxies = useMemo(
    () => (proxiesQuery.data ?? []).filter((proxy) => proxy.enabled),
    [proxiesQuery.data],
  );
  const mappedImportedProxyIds = useMemo(
    () =>
      new Set(
        importedProxies
          .map((proxy) => proxy.existing_proxy_id)
          .filter(Boolean),
      ),
    [importedProxies],
  );
  const validProxyIds = useMemo(
    () =>
      new Set([
        ...enabledProxies.map((proxy) => proxy.id),
        ...mappedImportedProxyIds,
      ]),
    [enabledProxies, mappedImportedProxyIds],
  );
  const selected = credentials.filter((credential) => credential.selected);
  const ready = selected.filter(
    (credential) => credentialErrors(credential, validProxyIds).length === 0,
  );
  const importedCount = credentials.filter(
    (credential) => credential.status === "imported",
  ).length;
  const failedCount = credentials.filter(
    (credential) => credential.status === "failed",
  ).length;
  const editing = credentials.find((credential) => credential.id === editingId);

  const parseDocuments = () => {
    const documents = [...uploadedDocuments];
    if (jsonText.trim()) {
      documents.unshift({
        name: "pasted-codex-credentials.json",
        content: jsonText,
      });
    }
    if (documents.length === 0) {
      toast.error(t("Paste JSON or choose at least one JSON file."));
      return;
    }
    const parsed = parseCodexImportDocuments(documents);
    const matchedProxies = parsed.proxies.map((proxy) => ({
      ...proxy,
      existing_proxy_id: matchExistingProxy(proxy, proxies)?.id ?? "",
    }));
    const mappedBySource = new Map(
      matchedProxies.map((proxy) => [
        proxy.source_key,
        proxy.existing_proxy_id,
      ]),
    );
    setImportedProxies(matchedProxies);
    setCredentials(
      parsed.credentials.map((credential) => ({
        ...credential,
        proxy_id:
          mappedBySource.get(credential.source_proxy_key) ??
          credential.proxy_id,
      })),
    );
    setParseErrors(parsed.errors);
    setSourceDirty(parsed.errors.length > 0);
    if (parsed.credentials.length === 0) {
      toast.error(t("No importable Codex credentials were found."));
    } else {
      toast.success(
        t("Parsed {count} credential drafts.", {
          count: parsed.credentials.length,
        }),
      );
    }
  };

  const chooseFiles = async (files: FileList | null) => {
    if (!files) return;
    const selectedFiles = Array.from(files).slice(0, MAX_FILES);
    const oversized = selectedFiles.find((file) => file.size > MAX_FILE_BYTES);
    if (oversized) {
      toast.error(
        t("{name} exceeds the 5 MiB file limit.", { name: oversized.name }),
      );
      return;
    }
    const documents = await Promise.all(
      selectedFiles.map(async (file) => ({
        name: file.name,
        content: await file.text(),
      })),
    );
    setUploadedDocuments(documents);
    setSourceDirty(true);
  };

  const patchCredential = (
    id: string,
    patch: Partial<CodexCredentialImportDraft>,
  ) => {
    setCredentials((current) =>
      current.map((credential) =>
        credential.id === id
          ? {
              ...credential,
              ...patch,
              status: patch.status ?? (
                credential.status === "imported"
                  ? credential.status
                  : "pending"),
              result_message: patch.result_message ?? (
                credential.status === "imported"
                  ? credential.result_message
                  : ""),
            }
          : credential,
      ),
    );
  };

  const assignImportedProxy = (sourceKey: string, proxyId: string) => {
    setImportedProxies((current) =>
      current.map((proxy) =>
        proxy.source_key === sourceKey
          ? { ...proxy, existing_proxy_id: proxyId }
          : proxy,
      ),
    );
    setCredentials((current) =>
      current.map((credential) =>
        credential.source_proxy_key === sourceKey
          ? { ...credential, proxy_id: proxyId }
          : credential,
      ),
    );
  };

  const applyBulkProxy = () => {
    const proxyId = bulkProxyId === NO_PROXY ? "" : bulkProxyId;
    setCredentials((current) =>
      current.map((credential) =>
        credential.selected ? { ...credential, proxy_id: proxyId } : credential,
      ),
    );
  };

  const assignRoundRobin = () => {
    if (enabledProxies.length === 0) {
      toast.error(t("Create or enable at least one proxy first."));
      return;
    }
    let index = 0;
    setCredentials((current) =>
      current.map((credential) => {
        if (!credential.selected) return credential;
        const proxy = enabledProxies[index % enabledProxies.length];
        index += 1;
        return { ...credential, proxy_id: proxy?.id ?? "" };
      }),
    );
  };

  const runImport = async () => {
    const targets = credentials.filter(
      (credential) =>
        credential.selected && credential.status !== "imported",
    );
    if (targets.length === 0) {
      toast.error(t("Select at least one pending credential."));
      return;
    }
    const invalid = targets.find(
      (credential) => credentialErrors(credential, validProxyIds).length > 0,
    );
    if (invalid) {
      toast.error(t("Fix validation errors before importing."));
      return;
    }

    setImporting(true);
    let success = 0;
    let failed = 0;
    for (const target of targets) {
      patchCredential(target.id, {
        status: "importing",
        result_message: "",
      });
      const input: CodexCredentialImportInput = {
        label: target.label.trim(),
        enabled: target.enabled,
        proxy_id: target.proxy_id || null,
        quota_threshold_percent: Number(target.quota_threshold_percent),
        access_token: target.access_token.trim(),
        refresh_token: target.refresh_token.trim(),
        account_id: target.account_id.trim() || null,
        user_id: target.user_id.trim() || null,
      };
      if (target.id_token.trim()) input.id_token = target.id_token.trim();
      try {
        await importCredential.mutateAsync(input);
        success += 1;
        patchCredential(target.id, {
          status: "imported",
          result_message: t("Imported"),
        });
      } catch (error) {
        failed += 1;
        patchCredential(target.id, {
          status: "failed",
          result_message: codexImportError(error, t),
        });
      }
    }
    setImporting(false);
    if (failed === 0) {
      toast.success(
        t("Imported {count} Codex credentials.", { count: success }),
      );
    } else {
      toast.warning(
        t("Import finished: {success} succeeded and {failed} failed.", {
          success,
          failed,
        }),
      );
    }
  };

  const clearSensitiveDrafts = () => {
    setSourceDirty(false);
    setJsonText("");
    setUploadedDocuments([]);
    setCredentials([]);
    setImportedProxies([]);
    setParseErrors([]);
    setEditingId(null);
  };

  return (
    <div className="flex flex-col gap-6">
      {navigationGuard}
      <PageHeader
        backTo={returnTo}
        title={t("Advanced Codex import")}
        actions={
          <>
            <Button
              variant="outline"
              onClick={clearSensitiveDrafts}
              disabled={
                importing &&
                credentials.some(
                  (credential) => credential.status === "importing",
                )
              }
            >
              <RotateCcw data-icon="inline-start" />
              {t("Clear drafts")}
            </Button>
          </>
        }
      />

      {proxiesQuery.error ? <ErrorAlert error={proxiesQuery.error} /> : null}

      <Alert>
        <FileJson />
        <AlertTitle>{t("Sensitive credential workspace")}</AlertTitle>
        <AlertDescription>
          {t(
            "Imported tokens remain only in this page's memory until submitted. Clear drafts when you finish, and protect exported JSON files as secrets.",
          )}
        </AlertDescription>
      </Alert>

      <Card>
        <CardHeader>
          <CardTitle>{t("1. Load JSON")}</CardTitle>
          <CardDescription>
            {t("Use a credential or array (including CLIProxyAPI), an ai-gateway v3 export, or a Sub2API export.")}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <Tabs defaultValue="paste">
            <TabsList>
              <TabsTrigger value="paste">{t("Paste JSON")}</TabsTrigger>
              <TabsTrigger value="files">{t("JSON files")}</TabsTrigger>
            </TabsList>
            <TabsContent value="paste">
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="codex-import-json">
                    {t("Credential JSON")}
                  </FieldLabel>
                  <Textarea
                    id="codex-import-json"
                    value={jsonText}
                    onChange={(event) => {
                      setJsonText(event.target.value);
                      setSourceDirty(Boolean(event.target.value.trim() || uploadedDocuments.length));
                    }}
                    rows={12}
                    spellCheck={false}
                    placeholder='{"type":"codex","access_token":"…","refresh_token":"…"}'
                    className="font-mono text-xs"
                  />
                </Field>
              </FieldGroup>
            </TabsContent>
            <TabsContent value="files">
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="codex-import-files">
                    {t("JSON files")}
                  </FieldLabel>
                  <Input
                    id="codex-import-files"
                    type="file"
                    accept=".json,application/json"
                    multiple
                    onChange={(event) => void chooseFiles(event.target.files)}
                  />
                  <FieldDescription>
                    {t("Up to 20 files, 5 MiB each.")}
                  </FieldDescription>
                </Field>
                {uploadedDocuments.length > 0 ? (
                  <div className="flex flex-wrap gap-2">
                    {uploadedDocuments.map((document) => (
                      <Badge key={document.name} variant="secondary">
                        {document.name}
                      </Badge>
                    ))}
                  </div>
                ) : null}
              </FieldGroup>
            </TabsContent>
          </Tabs>
          <div className="flex flex-wrap gap-2">
            <Button onClick={parseDocuments}>
              <WandSparkles data-icon="inline-start" />
              {t("Parse into drafts")}
            </Button>
            <Button
              variant="outline"
              onClick={() => {
                setUploadedDocuments([]);
                setJsonText("");
                setSourceDirty(false);
              }}
            >
              <Trash2 data-icon="inline-start" />
              {t("Clear sources")}
            </Button>
          </div>
          {parseErrors.length > 0 ? (
            <Alert variant="destructive">
              <AlertTitle>{t("Some input could not be parsed")}</AlertTitle>
              <AlertDescription>
                <ul className="list-disc pl-4">
                  {parseErrors.map((error) => (
                    <li key={error}>{t(error)}</li>
                  ))}
                </ul>
              </AlertDescription>
            </Alert>
          ) : null}
        </CardContent>
      </Card>

      <CodexImportProxyManager
        proxies={proxies}
        importedProxies={importedProxies}
        onAssignImportedProxy={assignImportedProxy}
        onProxyDeleted={(proxyId) => {
          setCredentials((current) =>
            current.map((credential) =>
              credential.proxy_id === proxyId
                ? { ...credential, proxy_id: "" }
                : credential,
            ),
          );
          setImportedProxies((current) =>
            current.map((proxy) =>
              proxy.existing_proxy_id === proxyId
                ? { ...proxy, existing_proxy_id: "" }
                : proxy,
            ),
          );
        }}
      />

      {credentials.length > 0 ? (
        <>
          <div className="grid gap-4 sm:grid-cols-4">
            <SummaryCard label={t("Drafts")} value={credentials.length} />
            <SummaryCard label={t("Selected")} value={selected.length} />
            <SummaryCard label={t("Imported")} value={importedCount} />
            <SummaryCard label={t("Failed")} value={failedCount} />
          </div>

          <Card>
            <CardHeader>
              <CardTitle>{t("2. Review credentials")}</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="flex flex-wrap items-end gap-3">
                <Field className="min-w-56">
                  <FieldLabel>{t("Bulk proxy assignment")}</FieldLabel>
                  <Select
                    value={bulkProxyId}
                    onValueChange={setBulkProxyId}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value={NO_PROXY}>{t("Direct")}</SelectItem>
                        {enabledProxies.map((proxy) => (
                          <SelectItem key={proxy.id} value={proxy.id}>
                            {proxy.name}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                <Button variant="outline" onClick={applyBulkProxy}>
                  {t("Apply to selected")}
                </Button>
                <Button variant="outline" onClick={assignRoundRobin}>
                  {t("Round-robin selected")}
                </Button>
              </div>

              <ResourceTable
                columns={[
                  {
                    key: "select",
                    header: (
                      <Checkbox
                        aria-label={t("Select all credentials")}
                        checked={
                          credentials.length > 0 &&
                          credentials.every((credential) => credential.selected)
                        }
                        onCheckedChange={(checked) =>
                          setCredentials((current) =>
                            current.map((credential) => ({
                              ...credential,
                              selected: Boolean(checked),
                            })),
                          )
                        }
                      />
                    ),
                    className: "align-top",
                    render: (credential) => {
                      const index = credentials.indexOf(credential);
                      return (
                        <Checkbox
                          aria-label={t("Select credential {number}", {
                            number: index + 1,
                          })}
                          checked={credential.selected}
                          disabled={credential.status === "imported"}
                          onCheckedChange={(checked) =>
                            patchCredential(credential.id, {
                              selected: Boolean(checked),
                            })
                          }
                        />
                      );
                    },
                  },
                  {
                    key: "credential",
                    header: t("Credential"),
                    className: "min-w-72 align-top",
                    render: (credential) => {
                      const index = credentials.indexOf(credential);
                      const errors = credentialErrors(credential, validProxyIds);
                      const warnings = credentialWarnings(credential, importedProxies);
                      return (
                        <>
                          <Input
                            aria-label={t("Label for credential {number}", {
                              number: index + 1,
                            })}
                            value={credential.label}
                            onChange={(event) =>
                              patchCredential(credential.id, {
                                label: event.target.value,
                              })
                            }
                          />
                          <div className="mt-2 flex flex-wrap gap-1">
                            <Badge variant="outline">
                              {credential.source}
                            </Badge>
                            {credential.email ? (
                              <Badge variant="secondary">
                                {credential.email}
                              </Badge>
                            ) : null}
                          </div>
                          <CodexAccountIdentity accountId={credential.account_id} userId={credential.user_id} />
                          {errors.map((error) => (
                            <p
                              className="mt-1 text-xs text-destructive"
                              key={error}
                            >
                              {t(error)}
                            </p>
                          ))}
                          {warnings.map((warning) => (
                            <p
                              className="mt-1 text-xs text-warning-foreground"
                              key={warning}
                            >
                              {t(warning)}
                            </p>
                          ))}
                        </>
                      );
                    },
                  },
                  {
                    key: "proxy",
                    header: t("Proxy"),
                    className: "min-w-56 align-top",
                    render: (credential) => (
                      <Select
                        value={credential.proxy_id || NO_PROXY}
                        onValueChange={(value) =>
                          patchCredential(credential.id, {
                            proxy_id: value === NO_PROXY ? "" : value,
                          })
                        }
                      >
                        <SelectTrigger
                          aria-label={t("Proxy for {label}", {
                            label: credential.label,
                          })}
                        >
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem value={NO_PROXY}>
                              {t("Direct")}
                            </SelectItem>
                            {enabledProxies.map((proxy) => (
                              <SelectItem key={proxy.id} value={proxy.id}>
                                {proxy.name}
                              </SelectItem>
                            ))}
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                    ),
                  },
                  {
                    key: "threshold",
                    header: t("Threshold"),
                    className: "min-w-28 align-top",
                    render: (credential) => (
                      <Input
                        aria-label={t("Threshold for {label}", {
                          label: credential.label,
                        })}
                        type="number"
                        min={1}
                        max={100}
                        value={credential.quota_threshold_percent}
                        onChange={(event) =>
                          patchCredential(credential.id, {
                            quota_threshold_percent: event.target.value,
                          })
                        }
                      />
                    ),
                  },
                  {
                    key: "status",
                    header: t("Status"),
                    className: "align-top",
                    render: (credential) => (
                      <>
                        <ImportStatusBadge credential={credential} />
                        {credential.result_message ? (
                          <p className="mt-2 max-w-48 text-xs text-muted-foreground">
                            {credential.result_message}
                          </p>
                        ) : null}
                      </>
                    ),
                  },
                  {
                    key: "actions",
                    header: t("Actions"),
                    pinned: true,
                    className: "align-top",
                    render: (credential) => (
                      <div className="flex justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("Edit tokens for {label}", {
                            label: credential.label,
                          })}
                          onClick={() => setEditingId(credential.id)}
                        >
                          <Pencil />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("Remove {label}", {
                            label: credential.label,
                          })}
                          onClick={() =>
                            setCredentials((current) =>
                              current.filter((item) => item.id !== credential.id),
                            )
                          }
                        >
                          <Trash2 />
                        </Button>
                      </div>
                    ),
                  },
                ] satisfies Column<CodexCredentialImportDraft>[]}
                rows={credentials}
                rowKey={(credential) => credential.id}
                pagination={false}
              />

              <div className="flex flex-wrap items-center justify-between gap-3">
                <p className="text-sm text-muted-foreground">
                  {t("{ready} of {selected} selected drafts are ready.", {
                    ready: ready.length,
                    selected: selected.length,
                  })}
                </p>
                <Button
                  onClick={() => void runImport()}
                  disabled={importing || ready.length === 0}
                >
                  {importing ? <Spinner data-icon="inline-start" /> : null}
                  <Upload data-icon="inline-start" />
                  {t("Validate and import selected")}
                </Button>
              </div>
            </CardContent>
          </Card>
        </>
      ) : null}

      <CredentialEditorDialog
        credential={editing ?? null}
        onChange={(patch) => {
          if (editing) patchCredential(editing.id, patch);
        }}
        onClose={() => setEditingId(null)}
      />
    </div>
  );
}
