import { useMemo, useState } from "react";
import {
  ArrowLeft,
  FileJson,
  Pencil,
  RotateCcw,
  Trash2,
  Upload,
  WandSparkles,
} from "lucide-react";
import { Link, useParams } from "react-router";
import { toast } from "sonner";
import type {
  CodexCredentialImportInput,
  ProxyView,
} from "@/api/types";
import { ApiError } from "@/api/errors";
import { useI18n } from "@/app/i18n";
import { ErrorAlert } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
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
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useChannelGroup, useProxies } from "@/features/admin/api";
import {
  type CodexCredentialImportDraft,
  type CodexImportDocument,
  type ImportedProxyDraft,
  parseCodexImportDocuments,
} from "./codex-import";
import { CodexImportProxyManager } from "./codex-import-proxy-manager";
import { useImportCodexCredential } from "./api";

const NO_PROXY = "__none__";
const MAX_FILE_BYTES = 5 * 1024 * 1024;
const MAX_FILES = 20;

export default function CodexImportPage() {
  const { t } = useI18n();
  const { id: groupId = "" } = useParams();
  const group = useChannelGroup(groupId);
  const proxiesQuery = useProxies();
  const importCredential = useImportCodexCredential(groupId);
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
              status:
                credential.status === "imported"
                  ? credential.status
                  : "pending",
              result_message:
                credential.status === "imported"
                  ? credential.result_message
                  : "",
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
        weight: Number(target.weight),
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
    setJsonText("");
    setUploadedDocuments([]);
    setCredentials([]);
    setImportedProxies([]);
    setParseErrors([]);
    setEditingId(null);
  };

  const groupError =
    group.data && group.data.data.connector_kind !== "codex_oauth"
      ? new Error(t("This channel group is not a Codex OAuth connector."))
      : null;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={t("Advanced Codex import")}
        description={t(
          "Parse native, CLIProxyAPI, and Sub2API JSON into editable drafts, configure proxies, then import.",
        )}
        actions={
          <>
            <Button
              variant="outline"
              render={
                <Link to={`/admin/providers/codex-oauth/${groupId}`} />
              }
              nativeButton={false}
            >
              <ArrowLeft data-icon="inline-start" />
              {t("Back to credentials")}
            </Button>
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

      {group.error ? <ErrorAlert error={group.error} /> : null}
      {groupError ? <ErrorAlert error={groupError} /> : null}
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
            {t(
              "Paste JSON, upload one or more .json files, or combine both sources. Format detection is automatic.",
            )}
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
                    onChange={(event) => setJsonText(event.target.value)}
                    rows={12}
                    spellCheck={false}
                    placeholder='{"type":"codex","access_token":"…","refresh_token":"…"}'
                    className="font-mono text-xs"
                  />
                  <FieldDescription>
                    {t(
                      "Supported roots include a single credential, an array, an ai-gateway bundle, or a Sub2API data export envelope.",
                    )}
                  </FieldDescription>
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
              <CardDescription>
                {t(
                  "Edit labels, routing values, tokens, and per-credential proxy assignments before the server validates each credential.",
                )}
              </CardDescription>
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

              <div className="overflow-x-auto rounded-xl border">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>
                        <Checkbox
                          aria-label={t("Select all credentials")}
                          checked={
                            credentials.length > 0 &&
                            credentials.every(
                              (credential) => credential.selected,
                            )
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
                      </TableHead>
                      <TableHead>{t("Credential")}</TableHead>
                      <TableHead>{t("Proxy")}</TableHead>
                      <TableHead>{t("Weight")}</TableHead>
                      <TableHead>{t("Threshold")}</TableHead>
                      <TableHead>{t("Status")}</TableHead>
                      <TableHead className="text-right">
                        {t("Actions")}
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {credentials.map((credential, index) => {
                      const errors = credentialErrors(
                        credential,
                        validProxyIds,
                      );
                      const warnings = credentialWarnings(
                        credential,
                        importedProxies,
                      );
                      return (
                        <TableRow key={credential.id}>
                          <TableCell className="align-top">
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
                          </TableCell>
                          <TableCell className="min-w-72 align-top">
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
                            <p className="mt-2 break-all text-xs text-muted-foreground">
                              {credential.account_id ||
                                t("Personal credential (no workspace ID)")}
                            </p>
                            {credential.user_id ? (
                              <p className="mt-1 break-all text-xs text-muted-foreground">
                                {t("Member {id}", { id: credential.user_id })}
                              </p>
                            ) : null}
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
                          </TableCell>
                          <TableCell className="min-w-56 align-top">
                            <Select
                              value={credential.proxy_id || NO_PROXY}
                              onValueChange={(value) =>
                                patchCredential(credential.id, {
                                  proxy_id:
                                    value === NO_PROXY ? "" : value,
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
                                    <SelectItem
                                      key={proxy.id}
                                      value={proxy.id}
                                    >
                                      {proxy.name}
                                    </SelectItem>
                                  ))}
                                </SelectGroup>
                              </SelectContent>
                            </Select>
                          </TableCell>
                          <TableCell className="min-w-28 align-top">
                            <Input
                              aria-label={t("Weight for {label}", {
                                label: credential.label,
                              })}
                              type="number"
                              min={1}
                              value={credential.weight}
                              onChange={(event) =>
                                patchCredential(credential.id, {
                                  weight: event.target.value,
                                })
                              }
                            />
                          </TableCell>
                          <TableCell className="min-w-28 align-top">
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
                                  quota_threshold_percent:
                                    event.target.value,
                                })
                              }
                            />
                          </TableCell>
                          <TableCell className="align-top">
                            <ImportStatusBadge credential={credential} />
                            {credential.result_message ? (
                              <p className="mt-2 max-w-48 text-xs text-muted-foreground">
                                {credential.result_message}
                              </p>
                            ) : null}
                          </TableCell>
                          <TableCell className="align-top">
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
                                    current.filter(
                                      (item) => item.id !== credential.id,
                                    ),
                                  )
                                }
                              >
                                <Trash2 />
                              </Button>
                            </div>
                          </TableCell>
                        </TableRow>
                      );
                    })}
                  </TableBody>
                </Table>
              </div>

              <div className="flex flex-wrap items-center justify-between gap-3">
                <p className="text-sm text-muted-foreground">
                  {t("{ready} of {selected} selected drafts are ready.", {
                    ready: ready.length,
                    selected: selected.length,
                  })}
                </p>
                <Button
                  onClick={() => void runImport()}
                  disabled={importing || ready.length === 0 || Boolean(groupError)}
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

function CredentialEditorDialog({
  credential,
  onChange,
  onClose,
}: {
  credential: CodexCredentialImportDraft | null;
  onChange: (patch: Partial<CodexCredentialImportDraft>) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  return (
    <Dialog
      open={Boolean(credential)}
      onOpenChange={(open) => !open && onClose()}
    >
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("Edit credential secrets")}</DialogTitle>
          <DialogDescription>
            {t(
              "Review or replace imported tokens and identity fields. Values remain in memory until import.",
            )}
          </DialogDescription>
        </DialogHeader>
        {credential ? (
          <FieldGroup>
            <Field orientation="horizontal">
              <FieldLabel htmlFor="codex-import-enabled">
                {t("Enabled after import")}
              </FieldLabel>
              <Switch
                id="codex-import-enabled"
                checked={credential.enabled}
                onCheckedChange={(enabled) =>
                  onChange({ enabled: Boolean(enabled) })
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-account-id">
                {t("Account ID override (optional)")}
              </FieldLabel>
              <Input
                id="codex-import-account-id"
                value={credential.account_id}
                onChange={(event) =>
                  onChange({ account_id: event.target.value })
                }
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-user-id">
                {t("User ID override (optional)")}
              </FieldLabel>
              <Input
                id="codex-import-user-id"
                value={credential.user_id}
                onChange={(event) => onChange({ user_id: event.target.value })}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-id-token">
                {t("ID token (optional)")}
              </FieldLabel>
              <Textarea
                id="codex-import-id-token"
                value={credential.id_token}
                onChange={(event) =>
                  onChange({ id_token: event.target.value })
                }
                rows={4}
                spellCheck={false}
                className="font-mono text-xs"
              />
              <FieldDescription>
                {t(
                  "When omitted, the gateway reads identity claims from the access token.",
                )}
              </FieldDescription>
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-access-token">
                {t("Access token")}
              </FieldLabel>
              <Textarea
                id="codex-import-access-token"
                value={credential.access_token}
                onChange={(event) =>
                  onChange({ access_token: event.target.value })
                }
                rows={5}
                spellCheck={false}
                className="font-mono text-xs"
                aria-invalid={!credential.access_token.trim()}
              />
              {!credential.access_token.trim() ? (
                <FieldError>{t("Access token is required.")}</FieldError>
              ) : null}
            </Field>
            <Field>
              <FieldLabel htmlFor="codex-import-refresh-token">
                {t("Refresh token")}
              </FieldLabel>
              <Textarea
                id="codex-import-refresh-token"
                value={credential.refresh_token}
                onChange={(event) =>
                  onChange({ refresh_token: event.target.value })
                }
                rows={5}
                spellCheck={false}
                className="font-mono text-xs"
                aria-invalid={!credential.refresh_token.trim()}
              />
              {!credential.refresh_token.trim() ? (
                <FieldError>{t("Refresh token is required.")}</FieldError>
              ) : null}
            </Field>
          </FieldGroup>
        ) : null}
        <DialogFooter>
          <Button onClick={onClose}>{t("Done")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function SummaryCard({ label, value }: { label: string; value: number }) {
  return (
    <Card>
      <CardHeader>
        <CardDescription>{label}</CardDescription>
        <CardTitle className="text-2xl">{value}</CardTitle>
      </CardHeader>
    </Card>
  );
}

function ImportStatusBadge({
  credential,
}: {
  credential: CodexCredentialImportDraft;
}) {
  const { t } = useI18n();
  if (credential.status === "imported") {
    return <Badge variant="success">{t("Imported")}</Badge>;
  }
  if (credential.status === "failed") {
    return <Badge variant="destructive">{t("Failed")}</Badge>;
  }
  if (credential.status === "importing") {
    return <Badge variant="info">{t("Importing")}</Badge>;
  }
  return <Badge variant="secondary">{t("Pending")}</Badge>;
}

function credentialErrors(
  credential: CodexCredentialImportDraft,
  validProxyIds: Set<string>,
): string[] {
  const errors = credential.errors.filter((error) =>
    error.startsWith("Duplicate of import row"),
  );
  const weight = Number(credential.weight);
  const threshold = Number(credential.quota_threshold_percent);
  if (!credential.label.trim()) errors.push("Label is required.");
  if (!credential.access_token.trim()) errors.push("Access token is required.");
  if (!credential.refresh_token.trim()) errors.push("Refresh token is required.");
  if (!Number.isInteger(weight) || weight <= 0) {
    errors.push("Weight must be a positive integer.");
  }
  if (!Number.isInteger(threshold) || threshold < 1 || threshold > 100) {
    errors.push("Quota threshold must be from 1 to 100.");
  }
  if (credential.proxy_id && !validProxyIds.has(credential.proxy_id)) {
    errors.push("The assigned proxy is missing or disabled.");
  }
  return [...new Set(errors)];
}

function credentialWarnings(
  credential: CodexCredentialImportDraft,
  importedProxies: ImportedProxyDraft[],
): string[] {
  const warnings = [...credential.warnings];
  if (
    credential.source_proxy_key &&
    !credential.proxy_id &&
    importedProxies.some(
      (proxy) => proxy.source_key === credential.source_proxy_key,
    )
  ) {
    warnings.push(
      "The source proxy is not mapped; this credential will use a direct connection.",
    );
  }
  return [...new Set(warnings)];
}

function matchExistingProxy(
  imported: ImportedProxyDraft,
  existing: ProxyView[],
): ProxyView | undefined {
  return (
    existing.find(
      (proxy) =>
        proxy.name === imported.name && proxy.proxy_url === imported.proxy_url,
    ) ??
    existing.find((proxy) => proxy.proxy_url === imported.proxy_url)
  );
}

function codexImportError(
  error: unknown,
  t: (key: string, values?: Record<string, string | number>) => string,
): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "codex_account_changed":
        return t("The supplied workspace or user ID does not match the token.");
      case "codex_network_policy_invalid":
        return t("The selected outbound proxy is unavailable.");
      case "codex_refresh_token_invalid":
        return t("The refresh token is no longer valid.");
      case "codex_models_invalid":
        return t("The account returned no supported Codex models.");
      case "codex_credential_invalid":
        return t("The credential tokens are invalid.");
    }
    return error.message;
  }
  return error instanceof Error ? error.message : t("Import failed");
}
