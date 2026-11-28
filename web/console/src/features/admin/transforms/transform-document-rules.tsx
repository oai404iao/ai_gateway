import { Braces, Trash2, WandSparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/app/i18n";
import {
  JSON_VALUE_TYPES,
  patchNeedsIndex,
  patchNeedsValue,
  patchRequiresExistingTarget,
  type HeaderRule,
  type HeaderRuleKind,
  type HeaderScope,
  type PatchConditionKind,
  type PatchRule,
  type PatchRuleKind,
  type PatchScope,
  type PatchValueMode,
} from "./transform-document-model";

export function sectionEmpty(title: string, description: string) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <Braces />
        </EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        <EmptyDescription>{description}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

export function HeaderRulesEditor({
  idPrefix,
  rules,
  scope,
  onChange,
}: {
  idPrefix: string;
  rules: HeaderRule[];
  scope: HeaderScope;
  onChange: (rules: HeaderRule[]) => void;
}) {
  const { t } = useI18n();
  const updateRule = (index: number, partial: Partial<HeaderRule>) => {
    onChange(rules.map((rule, ruleIndex) => (ruleIndex === index ? { ...rule, ...partial } : rule)));
  };

  if (rules.length === 0) {
    return sectionEmpty(
      scope === "request" ? t("No request-header rules") : t("No response-header rules"),
      scope === "request"
        ? t("Start with a header that identifies or adapts the upstream request.")
        : t("Add a safe response header for the client."),
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {rules.map((rule, index) => (
        <div key={rule.id} className="flex flex-col gap-4">
          {index > 0 ? <Separator /> : null}
          <FieldGroup>
            <Field orientation="responsive">
              <FieldLabel>{t("Operation")}</FieldLabel>
              <Select
                value={rule.kind}
                onValueChange={(value) => updateRule(index, { kind: value as HeaderRuleKind })}
              >
                <SelectTrigger aria-label={t("Header operation")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="set">{t("Set header")}</SelectItem>
                    <SelectItem value="remove">{t("Remove header")}</SelectItem>
                    <SelectItem value="rename">{t("Rename header")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field orientation="responsive">
              <FieldLabel htmlFor={`${idPrefix}-${rule.id}-name`}>{t("Header name")}</FieldLabel>
              <Input
                id={`${idPrefix}-${rule.id}-name`}
                value={rule.name}
                onChange={(event) => updateRule(index, { name: event.target.value })}
                placeholder="x-gateway-trace"
              />
            </Field>
            {rule.kind !== "remove" ? (
              <Field orientation="responsive">
                <FieldLabel htmlFor={`${idPrefix}-${rule.id}-value`}>
                  {rule.kind === "rename" ? t("New header name") : t("Header value")}
                </FieldLabel>
                <Input
                  id={`${idPrefix}-${rule.id}-value`}
                  value={rule.value}
                  onChange={(event) => updateRule(index, { value: event.target.value })}
                  placeholder={rule.kind === "rename" ? "x-client-trace" : "console"}
                />
              </Field>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="self-start"
              onClick={() => onChange(rules.filter((_, ruleIndex) => ruleIndex !== index))}
            >
              <Trash2 data-icon="inline-start" />
              {t("Delete rule")}
            </Button>
          </FieldGroup>
        </div>
      ))}
    </div>
  );
}

export function PatchRulesEditor({
  idPrefix,
  rules,
  scope,
  onChange,
}: {
  idPrefix: string;
  rules: PatchRule[];
  scope: PatchScope;
  onChange: (rules: PatchRule[]) => void;
}) {
  const { t } = useI18n();
  const updateRule = (index: number, partial: Partial<PatchRule>) => {
    onChange(rules.map((rule, ruleIndex) => (ruleIndex === index ? { ...rule, ...partial } : rule)));
  };

  if (rules.length === 0) {
    return sectionEmpty(
      scope === "request" ? t("No request-body rules") : t("No response-body rules"),
      scope === "request"
        ? t("Add a JSON Patch to adjust an upstream request.")
        : t("Add a JSON Patch for the selected streaming event."),
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {rules.map((rule, index) => (
        <div key={rule.id} className="flex flex-col gap-4">
          {index > 0 ? <Separator /> : null}
          <FieldGroup>
            <Field orientation="responsive">
              <FieldLabel>{t("Operation")}</FieldLabel>
              <Select
                value={rule.kind}
                onValueChange={(value) => {
                  const kind = value as PatchRuleKind;
                  updateRule(index, {
                    kind,
                    index: patchNeedsIndex(kind) ? rule.index : "",
                    value: patchNeedsValue(kind) ? rule.value : "",
                    valueMode: patchNeedsValue(kind) ? rule.valueMode : "literal",
                  });
                }}
              >
                <SelectTrigger aria-label={t("JSON Patch operation")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="add">{t("Add value")}</SelectItem>
                    <SelectItem value="replace">{t("Replace value")}</SelectItem>
                    <SelectItem value="remove">{t("Remove value")}</SelectItem>
                    <SelectItem value="array_append">{t("Append to array")}</SelectItem>
                    <SelectItem value="array_prepend">{t("Prepend to array")}</SelectItem>
                    <SelectItem value="array_insert">{t("Insert into array")}</SelectItem>
                    <SelectItem value="array_remove">{t("Remove array item")}</SelectItem>
                    <SelectItem value="merge">{t("Merge object")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field orientation="responsive">
              <FieldLabel htmlFor={`${idPrefix}-${rule.id}-path`}>{t("JSON pointer")}</FieldLabel>
              <Input
                id={`${idPrefix}-${rule.id}-path`}
                value={rule.path}
                onChange={(event) => updateRule(index, { path: event.target.value })}
                placeholder="/temperature"
                className="font-mono text-xs"
              />
            </Field>
            {patchNeedsIndex(rule.kind) ? (
              <Field orientation="responsive">
                <FieldLabel htmlFor={`${idPrefix}-${rule.id}-index`}>{t("Array index")}</FieldLabel>
                <Input
                  id={`${idPrefix}-${rule.id}-index`}
                  type="number"
                  min={0}
                  step={1}
                  value={rule.index}
                  onChange={(event) => updateRule(index, { index: event.target.value })}
                />
              </Field>
            ) : null}
            {patchNeedsValue(rule.kind) ? (
              <>
                <Field orientation="responsive">
                  <FieldLabel>{t("Value source")}</FieldLabel>
                  <Select
                    value={rule.valueMode}
                    onValueChange={(value) => {
                      const valueMode = value as PatchValueMode;
                      updateRule(index, {
                        valueMode,
                        value:
                          valueMode === "literal"
                            ? rule.valueMode === "literal"
                              ? rule.value
                              : "null"
                            : valueMode === "template"
                              ? rule.valueMode === "template"
                                ? rule.value
                                : "{{value}}"
                              : "",
                      });
                    }}
                  >
                    <SelectTrigger aria-label={t("Value source")}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        <SelectItem value="literal">{t("Literal JSON")}</SelectItem>
                        <SelectItem value="current">{t("Current path value")}</SelectItem>
                        <SelectItem value="template">{t("String template")}</SelectItem>
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
                {rule.valueMode === "literal" ? (
                  <Field>
                    <FieldLabel htmlFor={`${idPrefix}-${rule.id}-value`}>
                      {t("Value (JSON)")}
                    </FieldLabel>
                    <Textarea
                      id={`${idPrefix}-${rule.id}-value`}
                      rows={2}
                      value={rule.value}
                      onChange={(event) => updateRule(index, { value: event.target.value })}
                      className="font-mono text-xs"
                    />
                    <FieldDescription>
                      {t("Use valid JSON, including quotes around strings.")}
                    </FieldDescription>
                  </Field>
                ) : null}
                {rule.valueMode === "current" ? (
                  <Field>
                    <FieldDescription>
                      {t(
                        "Copies the value at this rule's target path before the operation runs.",
                      )}
                    </FieldDescription>
                  </Field>
                ) : null}
                {rule.valueMode === "template" ? (
                  <Field>
                    <FieldLabel htmlFor={`${idPrefix}-${rule.id}-template`}>
                      {t("String template")}
                    </FieldLabel>
                    <Textarea
                      id={`${idPrefix}-${rule.id}-template`}
                      rows={2}
                      value={rule.value}
                      onChange={(event) => updateRule(index, { value: event.target.value })}
                      placeholder="gateway-{{value}}"
                      className="font-mono text-xs"
                    />
                    <FieldDescription>
                      {t("Use {{value}} to interpolate the current value as text.")}
                    </FieldDescription>
                  </Field>
                ) : null}
              </>
            ) : null}
            <Field orientation="responsive">
              <FieldLabel>{t("Run when")}</FieldLabel>
              <Select
                value={rule.condition}
                onValueChange={(value) => {
                  const condition = value as PatchConditionKind;
                  updateRule(index, {
                    condition,
                    conditionValue:
                      condition === rule.condition
                        ? rule.conditionValue
                        : condition === "type"
                          ? "string"
                          : "",
                  });
                }}
              >
                <SelectTrigger aria-label={t("Run when")}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="always">{t("Always")}</SelectItem>
                    <SelectItem value="exists">{t("Path exists")}</SelectItem>
                    <SelectItem
                      value="missing"
                      disabled={patchRequiresExistingTarget(rule.kind)}
                    >
                      {t("Path is missing")}
                    </SelectItem>
                    <SelectItem value="type">{t("Path has JSON type")}</SelectItem>
                    <SelectItem value="equals">{t("Path equals JSON")}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            {rule.condition === "type" ? (
              <Field orientation="responsive">
                <FieldLabel>{t("JSON value type")}</FieldLabel>
                <Select
                  value={rule.conditionValue || "string"}
                  onValueChange={(conditionValue) => updateRule(index, { conditionValue })}
                >
                  <SelectTrigger aria-label={t("JSON value type")}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {JSON_VALUE_TYPES.map((type) => (
                        <SelectItem key={type} value={type}>
                          {type}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            {rule.condition === "equals" ? (
              <Field>
                <FieldLabel htmlFor={`${idPrefix}-${rule.id}-equals`}>
                  {t("Equals value (JSON)")}
                </FieldLabel>
                <Textarea
                  id={`${idPrefix}-${rule.id}-equals`}
                  rows={2}
                  value={rule.conditionValue}
                  onChange={(event) => updateRule(index, { conditionValue: event.target.value })}
                  className="font-mono text-xs"
                />
              </Field>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="self-start"
              onClick={() => onChange(rules.filter((_, ruleIndex) => ruleIndex !== index))}
            >
              <Trash2 data-icon="inline-start" />
              {t("Delete rule")}
            </Button>
          </FieldGroup>
        </div>
      ))}
    </div>
  );
}

export function ReferenceExample({
  title,
  description,
  document,
  onApply,
}: {
  title: string;
  description: string;
  document: Record<string, unknown>;
  onApply: () => void;
}) {
  const { t } = useI18n();
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <pre className="overflow-x-auto rounded-lg border p-3 font-mono text-xs">
          {JSON.stringify(document, null, 2)}
        </pre>
        <Button type="button" variant="outline" size="sm" className="self-start" onClick={onApply}>
          <WandSparkles data-icon="inline-start" />
          {t("Apply example")}
        </Button>
      </CardContent>
    </Card>
  );
}

