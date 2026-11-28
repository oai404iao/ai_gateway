import { useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpen,
  Braces,
  CircleAlert,
  FileJson2,
  Plus,
  Trash2,
} from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/app/i18n";
import type { ApiFormat, ApiOperation } from "@/api/types";
import { apiFormatLabel, API_FORMATS, operationApiFormat } from "@/lib/permissions";
import {
  documentVersion,
  emptyDocument,
  hasRules,
  newHeaderRule,
  newPatchRule,
  parseVisualDocument,
  ruleId,
  serializeDocument,
  sseEvents,
  supportsRequestJson,
  supportsStreaming,
  validateVisualDocument,
  type EditorMode,
  type TransformDocumentEditorProps,
  type VisualDocument,
} from "./transform-document-model";
import {
  HeaderRulesEditor,
  PatchRulesEditor,
  ReferenceExample,
  sectionEmpty,
} from "./transform-document-rules";

export function TransformDocumentEditor({
  value,
  onChange,
  operation,
  defaultApiFormat,
  preserveWhenBlank = false,
  onVisualValidationChange,
}: TransformDocumentEditorProps) {
  const { t } = useI18n();
  const fixedApiFormat = operation && operationApiFormat(operation);
  const fallbackApiFormat = fixedApiFormat ?? defaultApiFormat ?? API_FORMATS[0];
  const parsed = useMemo(
    () => parseVisualDocument(value, fallbackApiFormat, operation),
    [value, fallbackApiFormat, operation],
  );
  const [mode, setMode] = useState<EditorMode>("visual");
  const [document, setDocument] = useState<VisualDocument>(() =>
    "document" in parsed ? parsed.document : emptyDocument(fallbackApiFormat),
  );
  const [visualError, setVisualError] = useState<string | null>(
    "error" in parsed ? parsed.error : null,
  );
  const emittedValue = useRef<{ value: string; format: ApiFormat; operation?: ApiOperation } | null>(null);

  useEffect(() => {
    const emitted = emittedValue.current;
    if (emitted?.value === value && emitted.format === fallbackApiFormat && emitted.operation === operation) {
      return;
    }
    emittedValue.current = null;

    if ("error" in parsed) {
      setVisualError(parsed.error);
      onVisualValidationChange?.(operation ? parsed.error : null);
      return;
    }
    setDocument({
      ...parsed.document,
      apiFormat: fixedApiFormat ?? parsed.document.apiFormat,
    });
    const error = fixedApiFormat
      ? validateVisualDocument(parsed.document, fixedApiFormat, operation)
      : null;
    setVisualError(error);
    onVisualValidationChange?.(error);
  }, [fallbackApiFormat, fixedApiFormat, operation, onVisualValidationChange, parsed, value]);

  const apiFormat = fixedApiFormat ?? document.apiFormat;
  const allowRequestJson = supportsRequestJson(operation);
  const allowStreaming = supportsStreaming(apiFormat, operation);
  const activeDocumentVersion = documentVersion(document);
  const availableSseEvents = useMemo(
    () => sseEvents(apiFormat).filter((event) => !document.sse.some((rule) => rule.event === event)),
    [apiFormat, document.sse],
  );

  const commitVisualDocument = (next: VisualDocument, writeNoop = true) => {
    setDocument(next);
    const nextApiFormat = fixedApiFormat ?? next.apiFormat;
    const error = validateVisualDocument(next, nextApiFormat, operation);
    setVisualError(error);
    onVisualValidationChange?.(error);
    if (error || (!writeNoop && !hasRules(next))) return;

    const nextValue = serializeDocument(next, nextApiFormat);
    emittedValue.current = { value: nextValue, format: fallbackApiFormat, operation };
    onChange(nextValue);
  };

  const applyExample = (example: Record<string, unknown>) => {
    const nextValue = JSON.stringify(example, null, 2);
    const parsed = parseVisualDocument(nextValue, fallbackApiFormat);
    if ("error" in parsed) return;
    const nextDocument = {
      ...parsed.document,
      apiFormat: fixedApiFormat ?? parsed.document.apiFormat,
    };
    emittedValue.current = { value: nextValue, format: fallbackApiFormat, operation };
    setDocument(nextDocument);
    setVisualError(null);
    onVisualValidationChange?.(null);
    onChange(nextValue);
    setMode("visual");
  };

  const requestHeaderExample = {
    version: 1,
    api_format: apiFormat,
    request_headers: {
      set: {
        "x-gateway-source": "console",
      },
    },
  };
  const proxyHeaderFilterExample = {
    version: 1,
    api_format: apiFormat,
    request_headers: {
      remove: [
        "forwarded",
        "via",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-forwarded-port",
        "x-real-ip",
        "x-client-ip",
        "x-original-forwarded-for",
        "true-client-ip",
        "cf-connecting-ip",
        "cf-connecting-ipv6",
        "cf-pseudo-ipv4",
        "cf-ipcountry",
        "cf-ray",
        "cf-visitor",
      ],
    },
  };
  const requestBodyExample = {
    version: 1,
    api_format: apiFormat,
    request_json: [
      {
        op: "replace",
        path: "/temperature",
        value: 0.2,
      },
    ],
  };
  const arrayRewriteExample = {
    version: 2,
    api_format: apiFormat,
    request_json: [
      {
        op: "array_prepend",
        path: "/messages",
        value: {
          role: "system",
          content: "Follow the gateway policy.",
        },
        when: {
          type: "array",
        },
      },
    ],
  };
  const currentValueExample = {
    version: 2,
    api_format: apiFormat,
    request_json: [
      {
        op: "replace",
        path: "/metadata",
        value: {
          original: {
            $ref: "current",
          },
          gateway: "console",
        },
        when: {
          type: "object",
        },
      },
    ],
  };
  const conditionalMergeExample = {
    version: 2,
    api_format: apiFormat,
    request_json: [
      {
        op: "merge",
        path: "/metadata",
        value: {
          gateway: "console",
        },
        when: {
          type: "object",
        },
      },
    ],
  };
  const responseHeaderExample = {
    version: 1,
    api_format: apiFormat,
    response_headers: {
      set: {
        "x-gateway-transform": "enabled",
      },
    },
  };
  const responseBodyExample = {
    version: 1,
    api_format: apiFormat,
    sse:
      apiFormat === "open_ai_images"
        ? []
        : [
            apiFormat === "open_ai_chat_completions"
              ? {
                  event: "chat.completion.chunk",
                  json: [
                    {
                      op: "add",
                      path: "/choices/0/delta/gateway_trace",
                      value: "proxied",
                    },
                  ],
                }
              : {
                  event: "response.output_text.delta",
                  json: [
                    {
                      op: "add",
                      path: "/gateway_trace",
                      value: "proxied",
                    },
                  ],
                },
          ],
  };

  return (
    <Tabs value={mode} onValueChange={(next) => setMode(next as EditorMode)}>
      <TabsList aria-label={t("Transform editor views")}>
        <TabsTrigger value="visual">
          <Braces data-icon="inline-start" />
          {t("Visual editor")}
        </TabsTrigger>
        <TabsTrigger value="json">
          <FileJson2 data-icon="inline-start" />
          {t("JSON configuration")}
        </TabsTrigger>
        <TabsTrigger value="reference">
          <BookOpen data-icon="inline-start" />
          {t("Reference")}
        </TabsTrigger>
      </TabsList>

      {visualError && mode !== "reference" ? (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertTitle>{t("Review the transform rules")}</AlertTitle>
          <AlertDescription>{t(visualError)}</AlertDescription>
        </Alert>
      ) : null}
      {"document" in parsed && <TabsContent value="visual" className="flex flex-col gap-4">
        {preserveWhenBlank && !value.trim() ? (
          <Alert>
            <CircleAlert />
            <AlertTitle>{t("Stored transform is redacted")}</AlertTitle>
            <AlertDescription>
              {t(
                "The existing document is not returned by the API. Adding or applying a visual rule replaces it; leave this editor untouched to preserve it.",
              )}
            </AlertDescription>
          </Alert>
        ) : null}

        <Card>
          <CardHeader>
            <CardTitle>{t("Transform scope")}</CardTitle>
            <CardDescription>
              {t(
                "Templates run first, then channel overrides. Upstream authentication is applied last and cannot be changed by transform rules.",
              )}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <FieldGroup className="grid gap-5 xl:grid-cols-2">
              <Field>
                <FieldLabel>{t("Rule format")}</FieldLabel>
                {fixedApiFormat ? (
                  <Badge variant="secondary">{apiFormatLabel(fixedApiFormat)}</Badge>
                ) : (
                  <Select
                    value={document.apiFormat}
                    onValueChange={(next) => {
                      const nextApiFormat = next as ApiFormat;
                      if (
                        document.sse.length > 0 &&
                        nextApiFormat !== document.apiFormat
                      ) {
                        const message =
                          "Remove streaming response rules before changing the API format.";
                        setVisualError(message);
                        onVisualValidationChange?.(message);
                        return;
                      }
                      commitVisualDocument({ ...document, apiFormat: nextApiFormat }, false);
                    }}
                  >
                    <SelectTrigger aria-label={t("Rule format")}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {API_FORMATS.map((format) => (
                          <SelectItem key={format} value={format}>
                            {apiFormatLabel(format)}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                )}
                <FieldDescription>
                  {fixedApiFormat
                    ? t("The operation fixes this transform's API format.")
                    : t("Choose the API format before adding streaming response rules.")}
                </FieldDescription>
              </Field>
              <Field>
                <FieldLabel>{t("Transform DSL version")}</FieldLabel>
                <Badge variant="secondary">v{activeDocumentVersion}</Badge>
                <FieldDescription>
                  {activeDocumentVersion === 1
                    ? t("Version 1 uses standard JSON Patch operations.")
                    : t(
                        "Version 2 adds bounded array edits, shallow object merge, target-value references, and conditions.",
                      )}
                </FieldDescription>
              </Field>
            </FieldGroup>
          </CardContent>
        </Card>

        <div className="grid items-start gap-4 xl:grid-cols-2">
          <div className="flex flex-col gap-4">
            <Card>
              <CardHeader className="flex flex-row items-start justify-between gap-4">
                <div className="flex flex-col gap-1.5">
                  <CardTitle>{t("Request headers")}</CardTitle>
                  <CardDescription>
                    {t("Set, remove, or rename safe headers before the request reaches the upstream.")}
                  </CardDescription>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    commitVisualDocument({
                      ...document,
                      requestHeaders: [...document.requestHeaders, newHeaderRule()],
                    })
                  }
                >
                  <Plus data-icon="inline-start" />
                  {t("Add request header rule")}
                </Button>
              </CardHeader>
              <CardContent>
                <HeaderRulesEditor
                  idPrefix="request-header"
                  rules={document.requestHeaders}
                  scope="request"
                  onChange={(requestHeaders) => commitVisualDocument({ ...document, requestHeaders })}
                />
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-start justify-between gap-4">
                <div className="flex flex-col gap-1.5">
                  <CardTitle>{t("Request body (JSON Patch)")}</CardTitle>
                  <CardDescription>
                    {allowRequestJson ? t(
                      "Apply JSON Patch or version 2 array and object operations to the upstream JSON request. Model and stream remain protected.",
                    ) : t("This operation does not support request-body rules.")}
                  </CardDescription>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!allowRequestJson}
                  onClick={() =>
                    commitVisualDocument({
                      ...document,
                      requestJson: [...document.requestJson, newPatchRule()],
                    })
                  }
                >
                  <Plus data-icon="inline-start" />
                  {t("Add request body rule")}
                </Button>
              </CardHeader>
              <CardContent className="flex flex-col gap-4">
                {(allowRequestJson || document.requestJson.length > 0) && (
                  <PatchRulesEditor
                    idPrefix="request-json"
                    rules={document.requestJson}
                    scope="request"
                    onChange={(requestJson) => commitVisualDocument({ ...document, requestJson })}
                  />
                )}
              </CardContent>
            </Card>
          </div>

          <div className="flex flex-col gap-4">
            <Card>
              <CardHeader className="flex flex-row items-start justify-between gap-4">
                <div className="flex flex-col gap-1.5">
                  <CardTitle>{t("Response headers")}</CardTitle>
                  <CardDescription>
                    {t("Set, remove, or rename safe headers before the response is sent to the client.")}
                  </CardDescription>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    commitVisualDocument({
                      ...document,
                      responseHeaders: [...document.responseHeaders, newHeaderRule()],
                    })
                  }
                >
                  <Plus data-icon="inline-start" />
                  {t("Add response header rule")}
                </Button>
              </CardHeader>
              <CardContent>
                <HeaderRulesEditor
                  idPrefix="response-header"
                  rules={document.responseHeaders}
                  scope="response"
                  onChange={(responseHeaders) => commitVisualDocument({ ...document, responseHeaders })}
                />
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-start justify-between gap-4">
                <div className="flex flex-col gap-1.5">
                  <CardTitle>{t("Response body (streaming SSE)")}</CardTitle>
                  <CardDescription>
                    {allowStreaming || !operation ? t(
                      "Patch supported JSON Server-Sent Events. Non-streaming JSON response bodies always pass through unchanged.",
                    ) : t("This operation does not support streaming response rules.")}
                  </CardDescription>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!allowStreaming || availableSseEvents.length === 0}
                  onClick={() => {
                    const event = availableSseEvents[0];
                    if (!event) return;
                    commitVisualDocument({
                      ...document,
                      sse: [...document.sse, { id: ruleId(), event, patches: [] }],
                    });
                  }}
                >
                  <Plus data-icon="inline-start" />
                  {t("Add streaming response rule")}
                </Button>
              </CardHeader>
              <CardContent className="flex flex-col gap-4">
                {document.sse.length === 0
                  ? allowStreaming && sectionEmpty(
                      t("No response-body rules"),
                      t("Add a supported streaming event and JSON Patch operations."),
                    )
                  : document.sse.map((rule, index) => (
                      <Card key={rule.id}>
                        <CardHeader className="flex flex-row items-start justify-between gap-4">
                          <div className="flex flex-col gap-1.5">
                            <CardTitle>{t("Streaming event rule")} {index + 1}</CardTitle>
                            <CardDescription>
                              {t("Each event can have one rule with one or more JSON Patch operations.")}
                            </CardDescription>
                          </div>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() =>
                              commitVisualDocument({
                                ...document,
                                sse: document.sse.filter((_, ruleIndex) => ruleIndex !== index),
                              })
                            }
                          >
                            <Trash2 data-icon="inline-start" />
                            {t("Delete rule")}
                          </Button>
                        </CardHeader>
                        <CardContent className="flex flex-col gap-4">
                          <FieldGroup>
                            <Field>
                              <FieldLabel>{t("Event")}</FieldLabel>
                              <Select
                                value={rule.event}
                                onValueChange={(event) =>
                                  commitVisualDocument({
                                    ...document,
                                    sse: document.sse.map((item, ruleIndex) =>
                                      ruleIndex === index ? { ...item, event } : item,
                                    ),
                                  })
                                }
                              >
                                <SelectTrigger aria-label={t("Streaming response event")}>
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  <SelectGroup>
                                    {sseEvents(apiFormat).map((event) => (
                                      <SelectItem
                                        key={event}
                                        value={event}
                                        disabled={document.sse.some(
                                          (item, ruleIndex) =>
                                            ruleIndex !== index && item.event === event,
                                        )}
                                      >
                                        {event}
                                      </SelectItem>
                                    ))}
                                  </SelectGroup>
                                </SelectContent>
                              </Select>
                            </Field>
                          </FieldGroup>
                          <PatchRulesEditor
                            idPrefix={`sse-${rule.id}`}
                            rules={rule.patches}
                            scope="sse"
                            onChange={(patches) =>
                              commitVisualDocument({
                                ...document,
                                sse: document.sse.map((item, ruleIndex) =>
                                  ruleIndex === index ? { ...item, patches } : item,
                                ),
                              })
                            }
                          />
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            className="self-start"
                            disabled={!allowStreaming}
                            onClick={() =>
                              commitVisualDocument({
                                ...document,
                                sse: document.sse.map((item, ruleIndex) =>
                                  ruleIndex === index
                                    ? { ...item, patches: [...item.patches, newPatchRule()] }
                                    : item,
                                ),
                              })
                            }
                          >
                            <Plus data-icon="inline-start" />
                            {t("Add event patch")}
                          </Button>
                        </CardContent>
                      </Card>
                    ))}
              </CardContent>
            </Card>
          </div>
        </div>

        <div className="grid gap-4 xl:grid-cols-2">
          <Alert>
            <CircleAlert />
            <AlertTitle>{t("JSON Patch note")}</AlertTitle>
            <AlertDescription>
              {t(
                "Replace and remove require a path that already exists. Add also requires its parent path to exist.",
              )}
            </AlertDescription>
          </Alert>
          <Alert>
            <CircleAlert />
            <AlertTitle>{t("Version 2 safety boundary")}</AlertTitle>
            <AlertDescription>
              {t(
                "Array operations target an existing array, merge is shallow and targets an existing object, and {{value}} can only read this rule's target before it runs.",
              )}
            </AlertDescription>
          </Alert>
        </div>
      </TabsContent>}

      <TabsContent value="json">
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="transform-document-json">{t("Transform document JSON")}</FieldLabel>
            <FieldDescription>
              {t(
                "Use this mode for direct editing. Visual editor supports transform schema versions 1 and 2 and keeps its generated JSON synchronized.",
              )}
            </FieldDescription>
            <Textarea
              id="transform-document-json"
              rows={24}
              value={value}
              onChange={(event) => {
                emittedValue.current = null;
                onChange(event.target.value);
              }}
              className="font-mono text-xs"
              spellCheck={false}
            />
          </Field>
        </FieldGroup>
      </TabsContent>

      <TabsContent value="reference" className="flex flex-col gap-4">
        <Alert>
          <CircleAlert />
          <AlertTitle>{t("Transform rule limits")}</AlertTitle>
          <AlertDescription>
            {t(
              "Request rules cannot change authorization, cookies, host, content length, content encoding, accept encoding, model, or stream. Response rules cannot change content type, content length, encoding, or cookies. Hop-by-hop headers are always protected.",
            )}
          </AlertDescription>
        </Alert>
        <Alert>
          <CircleAlert />
          <AlertTitle>{t("Response-body scope")}</AlertTitle>
          <AlertDescription>
            {t(
              "Response-body patches run only for the listed streaming SSE events. They do not buffer or rewrite ordinary non-streaming JSON responses.",
            )}
          </AlertDescription>
        </Alert>
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <ReferenceExample
            title={t("Request header example")}
            description={t("Add an upstream marker without touching protected credentials.")}
            document={requestHeaderExample}
            onApply={() => applyExample(requestHeaderExample)}
          />
          <ReferenceExample
            title={t("Proxy and CDN header filter template")}
            description={t(
              "Remove forwarding metadata commonly added by CDNs, Caddy, and nginx. Review the list if your upstream relies on proxy headers.",
            )}
            document={proxyHeaderFilterExample}
            onApply={() => applyExample(proxyHeaderFilterExample)}
          />
          {allowRequestJson && <>
            <ReferenceExample
              title={t("Request body example")}
              description={t("Replace a value that the client request already contains.")}
              document={requestBodyExample}
              onApply={() => applyExample(requestBodyExample)}
            />
            <ReferenceExample
              title={t("Array rewrite example")}
              description={t("Prepend a system message only when the target path is an array.")}
              document={arrayRewriteExample}
              onApply={() => applyExample(arrayRewriteExample)}
            />
            <ReferenceExample
              title={t("Current-value reference example")}
              description={t("Keep the original target value inside a replacement object.")}
              document={currentValueExample}
              onApply={() => applyExample(currentValueExample)}
            />
            <ReferenceExample
              title={t("Conditional merge example")}
              description={t("Shallow-merge safe metadata only when the target is an object.")}
              document={conditionalMergeExample}
              onApply={() => applyExample(conditionalMergeExample)}
            />
          </>}
          <ReferenceExample
            title={t("Response header example")}
            description={t("Expose a safe marker to clients in the upstream response.")}
            document={responseHeaderExample}
            onApply={() => applyExample(responseHeaderExample)}
          />
          {allowStreaming && <ReferenceExample
            title={t("Streaming response example")}
            description={t("Add a non-envelope field to a supported streaming event.")}
            document={responseBodyExample}
            onApply={() => applyExample(responseBodyExample)}
          />}
        </div>
      </TabsContent>
    </Tabs>
  );
}
