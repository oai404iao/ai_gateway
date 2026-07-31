import { useState, type FormEvent } from "react";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  EraserIcon,
  PencilIcon,
  PlusIcon,
  SparklesIcon,
  Trash2Icon,
} from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
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
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { StringListField } from "@/components/shared/string-list-field";
import { ConfirmDialog } from "@/components/shared/confirm-dialog";
import {
  useClearSessionAffinityCache,
  useSessionAffinityCache,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import type {
  SystemSessionAffinityKeySource,
  SystemSessionAffinityRule,
  SystemSessionAffinitySettings,
} from "@/api/types";

interface SessionAffinityErrors {
  maxEntries?: string;
  defaultTtl?: string;
  rules?: string;
}

interface SessionAffinityCardProps {
  value: SystemSessionAffinitySettings;
  onChange: (value: SystemSessionAffinitySettings) => void;
  errors?: SessionAffinityErrors;
}

const EMPTY_RULE: SystemSessionAffinityRule = {
  name: "",
  enabled: true,
  api_formats: ["open_ai_responses"],
  model_regex: [],
  key_sources: [{ type: "json_pointer", pointer: "/prompt_cache_key" }],
  value_regex: null,
  ttl_seconds: null,
};

const SENSITIVE_HEADERS = new Set([
  "authorization",
  "proxy-authorization",
  "proxy-authenticate",
  "cookie",
  "set-cookie",
  "host",
  "content-length",
  "connection",
  "transfer-encoding",
  "keep-alive",
  "te",
  "trailer",
  "upgrade",
  "proxy-connection",
]);
const HEADER_NAME_PATTERN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

function cloneRule(rule: SystemSessionAffinityRule): SystemSessionAffinityRule {
  return {
    ...rule,
    api_formats: [...rule.api_formats],
    model_regex: [...rule.model_regex],
    key_sources: rule.key_sources.map((source) => ({ ...source })),
  };
}

function sourceValue(source: SystemSessionAffinityKeySource): string {
  return source.type === "request_header" ? source.name : source.pointer;
}

function sourceWithValue(
  type: SystemSessionAffinityKeySource["type"],
  value: string,
): SystemSessionAffinityKeySource {
  return type === "request_header"
    ? { type, name: value }
    : { type, pointer: value };
}

function uniqueRuleName(rules: SystemSessionAffinityRule[], base: string): string {
  const names = new Set(rules.map((rule) => rule.name.toLocaleLowerCase()));
  if (!names.has(base.toLocaleLowerCase())) return base;
  for (let index = 2; index < 1_000; index += 1) {
    const candidate = `${base}-${index}`;
    if (!names.has(candidate.toLocaleLowerCase())) return candidate;
  }
  return `${base}-${Date.now()}`;
}

function codexRule(rules: SystemSessionAffinityRule[]): SystemSessionAffinityRule {
  return {
    name: uniqueRuleName(rules, "codex-responses"),
    enabled: true,
    api_formats: ["open_ai_responses"],
    model_regex: ["^gpt-.*$"],
    key_sources: [
      { type: "json_pointer", pointer: "/prompt_cache_key" },
      { type: "request_header", name: "session_id" },
      { type: "request_header", name: "thread_id" },
    ],
    value_regex: null,
    ttl_seconds: null,
  };
}

function moveItem<T>(items: T[], index: number, offset: number): T[] {
  const target = index + offset;
  if (target < 0 || target >= items.length) return items;
  const next = [...items];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

function validJsonPointer(pointer: string): boolean {
  if (!pointer.startsWith("/")) return false;
  for (let index = 0; index < pointer.length; index += 1) {
    if (pointer[index] !== "~") continue;
    index += 1;
    if (index >= pointer.length || (pointer[index] !== "0" && pointer[index] !== "1")) {
      return false;
    }
  }
  return true;
}

function validateRule(
  rule: SystemSessionAffinityRule,
  existing: SystemSessionAffinityRule[],
  editingIndex: number | null,
): string | null {
  const name = rule.name.trim();
  if (!name) return "Rule name is required.";
  if (name.length > 64) return "Rule name must be at most 64 characters.";
  if (
    existing.some(
      (candidate, index) =>
        index !== editingIndex &&
        candidate.name.trim().toLocaleLowerCase() === name.toLocaleLowerCase(),
    )
  ) {
    return "Rule names must be unique.";
  }
  if (rule.api_formats.length === 0) return "Pick at least one format.";
  if (rule.key_sources.length === 0) return "Add at least one key source.";
  if (rule.key_sources.length > 8) return "A rule can contain at most 8 key sources.";
  if (rule.model_regex.length > 8) return "A rule can contain at most 8 model expressions.";
  if (rule.model_regex.some((pattern) => !pattern || pattern.length > 256)) {
    return "Model regular expressions must contain 1 through 256 characters.";
  }
  if (rule.value_regex !== null && (!rule.value_regex || rule.value_regex.length > 256)) {
    return "Value regular expression must contain 1 through 256 characters.";
  }
  if (
    rule.ttl_seconds !== null &&
    (!Number.isInteger(rule.ttl_seconds) || rule.ttl_seconds < 1 || rule.ttl_seconds > 604_800)
  ) {
    return "Rule TTL must be between 1 and 604800 seconds.";
  }
  for (const source of rule.key_sources) {
    const value = sourceValue(source).trim();
    if (!value || value.length > 256) return "Every key source must contain a value.";
    if (source.type === "json_pointer" && !validJsonPointer(value)) {
      return "Enter a valid RFC 6901 JSON pointer.";
    }
    if (source.type === "request_header") {
      if (!HEADER_NAME_PATTERN.test(value)) return "Enter a valid request header name.";
      if (SENSITIVE_HEADERS.has(value.toLocaleLowerCase())) {
        return "Sensitive request headers cannot be affinity sources.";
      }
    }
  }
  return null;
}

export function SessionAffinityCard({
  value,
  onChange,
  errors,
}: SessionAffinityCardProps) {
  const { t } = useI18n();
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [draft, setDraft] = useState<SystemSessionAffinityRule>(() => cloneRule(EMPTY_RULE));
  const [editorError, setEditorError] = useState<string | null>(null);
  const [cacheClearTarget, setCacheClearTarget] = useState<{
    ruleName?: string;
  } | null>(null);
  const cache = useSessionAffinityCache();
  const clearCache = useClearSessionAffinityCache();
  const cacheCounts = new Map(
    cache.data?.rules.map((rule) => [rule.name, rule.entries]) ?? [],
  );

  const update = (patch: Partial<SystemSessionAffinitySettings>) =>
    onChange({ ...value, ...patch });

  const openNew = () => {
    setEditingIndex(null);
    setDraft(cloneRule(EMPTY_RULE));
    setEditorError(null);
    setEditorOpen(true);
  };

  const openEdit = (index: number) => {
    setEditingIndex(index);
    setDraft(cloneRule(value.rules[index]));
    setEditorError(null);
    setEditorOpen(true);
  };

  const saveRule = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const normalized = {
      ...draft,
      name: draft.name.trim(),
      model_regex: draft.model_regex.map((pattern) => pattern.trim()).filter(Boolean),
      key_sources: draft.key_sources.map((source) =>
        sourceWithValue(source.type, sourceValue(source).trim()),
      ),
      value_regex: draft.value_regex?.trim() || null,
    };
    const error = validateRule(normalized, value.rules, editingIndex);
    if (error) {
      setEditorError(error);
      return;
    }
    const rules = [...value.rules];
    if (editingIndex === null) {
      rules.push(normalized);
    } else {
      rules[editingIndex] = normalized;
    }
    update({ rules });
    setEditorOpen(false);
  };

  const addSource = () =>
    setDraft((current) => ({
      ...current,
      key_sources: [...current.key_sources, { type: "request_header", name: "" }],
    }));

  const confirmClearCache = () => {
    const target = cacheClearTarget;
    if (!target) return;
    setCacheClearTarget(null);
    clearCache.mutate(target.ruleName, {
      onSuccess: (response) => {
        toast.success(
          t("Cleared {count} cached entries.", {
            count: response.cleared_entries,
          }),
        );
      },
      onError: (error) => {
        toast.error(error instanceof Error ? error.message : t("Request failed"));
      },
    });
  };

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>{t("Session affinity")}</CardTitle>
          <CardDescription>
            {t(
              "Reuse the last successful channel for matching session keys without adding retries.",
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <FieldGroup className="grid gap-5 xl:grid-cols-2">
            <Field className="xl:col-span-2" orientation="horizontal">
              <FieldContent>
                <FieldLabel htmlFor="session_affinity_enabled">
                  {t("Enable session affinity")}
                </FieldLabel>
                <FieldDescription>
                  {t(
                    "The cache is process-local, bounded, and stores only hashed keys with channel IDs.",
                  )}
                </FieldDescription>
              </FieldContent>
              <Switch
                id="session_affinity_enabled"
                checked={value.enabled}
                onCheckedChange={(checked) => update({ enabled: Boolean(checked) })}
              />
            </Field>

            <Field data-invalid={Boolean(errors?.maxEntries)}>
              <FieldLabel htmlFor="session_affinity_max_entries">
                {t("Maximum cache entries")}
              </FieldLabel>
              <Input
                id="session_affinity_max_entries"
                type="number"
                min={1}
                max={1_000_000}
                value={Number.isNaN(value.max_entries) ? "" : value.max_entries}
                aria-invalid={Boolean(errors?.maxEntries)}
                onChange={(event) => update({ max_entries: event.target.valueAsNumber })}
              />
              <FieldDescription>
                {t("Bounds memory use when clients continuously generate new session keys.")}
              </FieldDescription>
              {errors?.maxEntries ? <FieldError>{t(errors.maxEntries)}</FieldError> : null}
            </Field>

            <Field data-invalid={Boolean(errors?.defaultTtl)}>
              <FieldLabel htmlFor="session_affinity_default_ttl">
                {t("Default affinity TTL (seconds)")}
              </FieldLabel>
              <Input
                id="session_affinity_default_ttl"
                type="number"
                min={1}
                max={604_800}
                value={
                  Number.isNaN(value.default_ttl_seconds) ? "" : value.default_ttl_seconds
                }
                aria-invalid={Boolean(errors?.defaultTtl)}
                onChange={(event) => update({ default_ttl_seconds: event.target.valueAsNumber })}
              />
              <FieldDescription>
                {t("Successful requests refresh the matched rule's TTL.")}
              </FieldDescription>
              {errors?.defaultTtl ? <FieldError>{t(errors.defaultTtl)}</FieldError> : null}
            </Field>

            <Field
              className="xl:col-span-2"
              data-invalid={Boolean(errors?.rules)}
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex flex-col gap-1">
                  <FieldLabel>{t("Affinity rules")}</FieldLabel>
                  <FieldDescription>
                    {t("Rules are evaluated from top to bottom; the first extracted key wins.")}
                  </FieldDescription>
                </div>
                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    disabled={
                      clearCache.isPending || (cache.data?.total_entries ?? 0) === 0
                    }
                    onClick={() => setCacheClearTarget({})}
                  >
                    <EraserIcon data-icon="inline-start" />
                    {t("Clear all cache ({count})", {
                      count: cache.data?.total_entries ?? 0,
                    })}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={value.rules.length >= 64}
                    onClick={() => update({ rules: [...value.rules, codexRule(value.rules)] })}
                  >
                    <SparklesIcon data-icon="inline-start" />
                    {t("Add Codex template")}
                  </Button>
                  <Button type="button" disabled={value.rules.length >= 64} onClick={openNew}>
                    <PlusIcon data-icon="inline-start" />
                    {t("Add rule")}
                  </Button>
                </div>
              </div>

              {value.rules.length === 0 ? (
                <Empty>
                  <EmptyHeader>
                    <EmptyTitle>{t("No affinity rules")}</EmptyTitle>
                    <EmptyDescription>
                      {t("Add a rule or start from the Codex Responses template.")}
                    </EmptyDescription>
                  </EmptyHeader>
                  <EmptyContent>
                  <Button type="button" variant="outline" onClick={openNew}>
                      <PlusIcon data-icon="inline-start" />
                      {t("Add rule")}
                    </Button>
                  </EmptyContent>
                </Empty>
              ) : (
                <Table className="min-w-max">
                  <TableHeader>
                    <TableRow>
                      <TableHead>{t("Rule")}</TableHead>
                      <TableHead>{t("Formats")}</TableHead>
                      <TableHead>{t("Key sources")}</TableHead>
                      <TableHead>{t("TTL")}</TableHead>
                      <TableHead>{t("Valid cache entries")}</TableHead>
                      <TableHead className="text-right">{t("Actions")}</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {value.rules.map((rule, index) => (
                      <TableRow key={`${rule.name}-${index}`}>
                        <TableCell>
                          <div className="flex items-center gap-2">
                            <span>{rule.name}</span>
                            <Badge variant={rule.enabled ? "default" : "secondary"}>
                              {t(rule.enabled ? "Enabled" : "Disabled")}
                            </Badge>
                          </div>
                        </TableCell>
                        <TableCell>
                          <div className="flex flex-wrap gap-1">
                            {rule.api_formats.map((format) => (
                              <Badge key={format} variant="outline">
                                {t(
                                  format === "open_ai_responses"
                                    ? "Responses"
                                    : "Chat Completions",
                                )}
                              </Badge>
                            ))}
                          </div>
                        </TableCell>
                        <TableCell>{rule.key_sources.length}</TableCell>
                        <TableCell>
                          {rule.ttl_seconds === null
                            ? t("Default")
                            : `${rule.ttl_seconds}s`}
                        </TableCell>
                        <TableCell>
                          <Badge variant="secondary">
                            {cache.isLoading
                              ? "…"
                              : (cacheCounts.get(rule.name) ?? 0).toLocaleString()}
                          </Badge>
                        </TableCell>
                        <TableCell>
                          <div className="flex justify-end gap-1">
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Clear cache for {name}", {
                                name: rule.name,
                              })}
                              disabled={
                                clearCache.isPending ||
                                (cacheCounts.get(rule.name) ?? 0) === 0
                              }
                              onClick={() =>
                                setCacheClearTarget({ ruleName: rule.name })
                              }
                            >
                              <EraserIcon />
                            </Button>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Move rule up")}
                              disabled={index === 0}
                              onClick={() => update({ rules: moveItem(value.rules, index, -1) })}
                            >
                              <ArrowUpIcon />
                            </Button>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Move rule down")}
                              disabled={index === value.rules.length - 1}
                              onClick={() => update({ rules: moveItem(value.rules, index, 1) })}
                            >
                              <ArrowDownIcon />
                            </Button>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Edit rule")}
                              onClick={() => openEdit(index)}
                            >
                              <PencilIcon />
                            </Button>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-sm"
                              aria-label={t("Delete rule")}
                              onClick={() =>
                                update({
                                  rules: value.rules.filter(
                                    (_, ruleIndex) => ruleIndex !== index,
                                  ),
                                })
                              }
                            >
                              <Trash2Icon />
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
              {errors?.rules ? <FieldError>{t(errors.rules)}</FieldError> : null}
            </Field>
          </FieldGroup>
        </CardContent>
      </Card>

      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto sm:max-w-3xl">
          <form onSubmit={saveRule}>
            <DialogHeader>
              <DialogTitle>{t(editingIndex === null ? "Add affinity rule" : "Edit affinity rule")}</DialogTitle>
              <DialogDescription>
                {t(
                  "A rule extracts one bounded scalar key and scopes it automatically by API key and model rule.",
                )}
              </DialogDescription>
            </DialogHeader>

            <FieldGroup className="grid gap-5 py-4 md:grid-cols-2">
              {editorError ? (
                <Alert className="md:col-span-2" variant="destructive">
                  <AlertTitle>{t("Review the rule")}</AlertTitle>
                  <AlertDescription>{t(editorError)}</AlertDescription>
                </Alert>
              ) : null}

              <Field>
                <FieldLabel htmlFor="affinity_rule_name">{t("Rule name")}</FieldLabel>
                <Input
                  id="affinity_rule_name"
                  value={draft.name}
                  maxLength={64}
                  onChange={(event) =>
                    setDraft((current) => ({ ...current, name: event.target.value }))
                  }
                />
              </Field>

              <Field orientation="horizontal">
                <FieldContent>
                  <FieldLabel htmlFor="affinity_rule_enabled">{t("Enable rule")}</FieldLabel>
                  <FieldDescription>
                    {t("Disabled rules remain saved but are not compiled into the data plane.")}
                  </FieldDescription>
                </FieldContent>
                <Switch
                  id="affinity_rule_enabled"
                  checked={draft.enabled}
                  onCheckedChange={(checked) =>
                    setDraft((current) => ({ ...current, enabled: Boolean(checked) }))
                  }
                />
              </Field>

              <FieldSet>
                <FieldLegend variant="label">{t("API formats")}</FieldLegend>
                <ToggleGroup
                  multiple
                  variant="outline"
                  value={draft.api_formats}
                  onValueChange={(formats) =>
                    setDraft((current) => ({
                      ...current,
                      api_formats: formats as SystemSessionAffinityRule["api_formats"],
                    }))
                  }
                >
                  <ToggleGroupItem value="open_ai_chat_completions">
                    {t("Chat Completions")}
                  </ToggleGroupItem>
                  <ToggleGroupItem value="open_ai_responses">{t("Responses")}</ToggleGroupItem>
                </ToggleGroup>
              </FieldSet>

              <StringListField
                id="affinity_model_regex"
                variant="tokens"
                label={t("Model regular expressions")}
                value={draft.model_regex}
                onChange={(model_regex) =>
                  setDraft((current) => ({ ...current, model_regex }))
                }
                placeholder="^gpt-.*$"
                description={t("Leave empty to match every model in the selected formats.")}
              />

              <Field>
                <FieldLabel htmlFor="affinity_value_regex">
                  {t("Value regular expression")}
                </FieldLabel>
                <Input
                  id="affinity_value_regex"
                  value={draft.value_regex ?? ""}
                  placeholder={t("Optional")}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      value_regex: event.target.value || null,
                    }))
                  }
                />
                <FieldDescription>
                  {t("Optional filter applied after a key source extracts a value.")}
                </FieldDescription>
              </Field>

              <Field>
                <FieldLabel htmlFor="affinity_rule_ttl">{t("Rule TTL (seconds)")}</FieldLabel>
                <Input
                  id="affinity_rule_ttl"
                  type="number"
                  min={1}
                  max={604_800}
                  value={draft.ttl_seconds ?? ""}
                  placeholder={t("Use default")}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      ttl_seconds:
                        event.target.value === "" ? null : event.target.valueAsNumber,
                    }))
                  }
                />
              </Field>

              <FieldSet className="md:col-span-2">
                <FieldLegend variant="label">{t("Key sources")}</FieldLegend>
                <FieldDescription>
                  {t("Sources are tried in order until one yields a non-empty scalar value.")}
                </FieldDescription>
                <FieldGroup>
                  {draft.key_sources.map((source, index) => (
                    <Field key={`${source.type}-${index}`} orientation="responsive">
                      <Select
                        value={source.type}
                        onValueChange={(type) =>
                          setDraft((current) => ({
                            ...current,
                            key_sources: current.key_sources.map((candidate, sourceIndex) =>
                              sourceIndex === index
                                ? sourceWithValue(
                                    type as SystemSessionAffinityKeySource["type"],
                                    "",
                                  )
                                : candidate,
                            ),
                          }))
                        }
                      >
                        <SelectTrigger className="w-40">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectGroup>
                            <SelectItem value="request_header">{t("Request header")}</SelectItem>
                            <SelectItem value="json_pointer">{t("JSON pointer")}</SelectItem>
                          </SelectGroup>
                        </SelectContent>
                      </Select>
                      <Input
                        aria-label={t("Key source value")}
                        value={sourceValue(source)}
                        placeholder={
                          source.type === "request_header"
                            ? "session_id"
                            : "/prompt_cache_key"
                        }
                        onChange={(event) =>
                          setDraft((current) => ({
                            ...current,
                            key_sources: current.key_sources.map((candidate, sourceIndex) =>
                              sourceIndex === index
                                ? sourceWithValue(source.type, event.target.value)
                                : candidate,
                            ),
                          }))
                        }
                      />
                      <div className="flex gap-1">
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("Move source up")}
                          disabled={index === 0}
                          onClick={() =>
                            setDraft((current) => ({
                              ...current,
                              key_sources: moveItem(current.key_sources, index, -1),
                            }))
                          }
                        >
                          <ArrowUpIcon />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("Move source down")}
                          disabled={index === draft.key_sources.length - 1}
                          onClick={() =>
                            setDraft((current) => ({
                              ...current,
                              key_sources: moveItem(current.key_sources, index, 1),
                            }))
                          }
                        >
                          <ArrowDownIcon />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t("Delete source")}
                          onClick={() =>
                            setDraft((current) => ({
                              ...current,
                              key_sources: current.key_sources.filter(
                                (_, sourceIndex) => sourceIndex !== index,
                              ),
                            }))
                          }
                        >
                          <Trash2Icon />
                        </Button>
                      </div>
                    </Field>
                  ))}
                  <Button
                    type="button"
                    variant="outline"
                    className="self-start"
                    disabled={draft.key_sources.length >= 8}
                    onClick={addSource}
                  >
                    <PlusIcon data-icon="inline-start" />
                    {t("Add key source")}
                  </Button>
                </FieldGroup>
              </FieldSet>
            </FieldGroup>

            <DialogFooter>
              <DialogClose
                render={<Button type="button" variant="outline" />}
              >
                {t("Cancel")}
              </DialogClose>
              <Button type="submit">{t("Save rule")}</Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={cacheClearTarget !== null}
        onOpenChange={(open) => {
          if (!open) setCacheClearTarget(null);
        }}
        title={t("Clear session affinity cache?")}
        description={
          cacheClearTarget?.ruleName
            ? t("This clears all valid cached bindings for rule {name}.", {
                name: cacheClearTarget.ruleName,
              })
            : t("This clears every valid session affinity binding in this process.")
        }
        confirmLabel={t("Clear cache")}
        destructive
        onConfirm={confirmClearCache}
      />
    </>
  );
}
