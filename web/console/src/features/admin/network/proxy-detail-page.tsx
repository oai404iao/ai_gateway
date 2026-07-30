import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { z } from "zod";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Spinner } from "@/components/ui/spinner";
import { AdminDetailShell } from "@/features/admin/components/admin-detail-shell";
import { DetailField } from "@/components/shared/detail-field";
import { StringListField } from "@/components/shared/string-list-field";
import { StatusBadge } from "@/components/shared/status-badge";
import { useCreateProxy, useProxy, useTestProxy, useUpdateProxy } from "@/features/admin/api";
import { ApiError, controlPlaneMutationErrorMessage } from "@/api/errors";
import type {
  ProxyCreateInput,
  ProxyInput,
  ProxyTestInput,
  ProxyTestResponse,
} from "@/api/types";
import { useI18n } from "@/app/i18n";
import { formatBoolean, formatDurationMs } from "@/lib/formatters";

function isAllowedProxyUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      ["http:", "https:", "socks4:", "socks4a:", "socks5:", "socks5h:"].includes(url.protocol) &&
      Boolean(url.hostname) &&
      !url.username &&
      !url.password &&
      (url.pathname === "" || url.pathname === "/") &&
      !url.search &&
      !url.hash
    );
  } catch {
    return false;
  }
}

const schema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  proxy_url: z
    .string()
    .trim()
    .refine(
      isAllowedProxyUrl,
      "Enter an HTTP(S) or SOCKS proxy URL without embedded credentials, path, query, or fragment.",
    ),
  username: z.string().nullable(),
  password: z.string().nullable(),
  no_proxy_hosts: z.array(z.string()),
  enabled: z.boolean(),
});
const testSchema = schema.pick({ proxy_url: true });

type FormState = z.infer<typeof schema>;

const empty: FormState = {
  name: "",
  proxy_url: "",
  username: null,
  password: null,
  no_proxy_hosts: [],
  enabled: true,
};

export function ProxyDetailPage() {
  const { id = "" } = useParams();
  const isNew = id === "new";
  const navigate = useNavigate();
  const { data, etag, isLoading, error } = useProxy(id);
  const create = useCreateProxy();
  const update = useUpdateProxy(id);
  const testProxy = useTestProxy();
  const { t } = useI18n();
  const [state, setState] = useState<FormState>(empty);
  const [testResult, setTestResult] = useState<ProxyTestResponse | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [validation, setValidation] = useState<z.ZodError | null>(null);

  useEffect(() => {
    if (data) {
      setState({
        name: data.data.name,
        proxy_url: data.data.proxy_url,
        username: null,
        password: null,
        no_proxy_hosts: data.data.no_proxy_hosts,
        enabled: data.data.enabled,
      });
    }
  }, [data]);

  const patch = (partial: Partial<FormState>) => {
    setState((prev) => ({ ...prev, ...partial }));
    setTestResult(null);
  };

  const runTest = async () => {
    const parsed = testSchema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    const input: ProxyTestInput = {
      proxy_url: parsed.data.proxy_url,
    };
    if (!isNew) input.proxy_id = id;
    if (state.username !== null) input.username = state.username;
    if (state.password !== null) input.password = state.password;

    try {
      const result = await testProxy.mutateAsync(input);
      setTestResult(result);
      toast.success(t("Proxy test succeeded"));
    } catch (error) {
      setTestResult(null);
      if (error instanceof ApiError && error.code === "proxy_test_invalid_configuration") {
        toast.error(t("The proxy settings are invalid."));
      } else if (
        error instanceof ApiError &&
        error.code === "proxy_test_credentials_required"
      ) {
        toast.error(t("Re-enter credentials before testing a changed proxy endpoint."));
      } else if (error instanceof ApiError && error.code === "proxy_test_rate_limited") {
        toast.error(t("ip-api.com rate limit reached. Try again after the reset window."));
      } else if (error instanceof ApiError && error.code === "proxy_test_timeout") {
        toast.error(t("The proxy test timed out."));
      } else {
        toast.error(t("Proxy test failed. Verify the proxy endpoint and credentials."));
      }
    }
  };

  const submit = async () => {
    const parsed = schema.safeParse(state);
    if (!parsed.success) {
      setValidation(parsed.error);
      return;
    }
    setValidation(null);
    setSubmitting(true);
    try {
      if (isNew) {
        const input: ProxyCreateInput = {
          name: parsed.data.name,
          proxy_url: parsed.data.proxy_url,
          username: parsed.data.username,
          password: parsed.data.password,
          no_proxy_hosts: parsed.data.no_proxy_hosts,
          enabled: parsed.data.enabled,
        };
        await create.mutateAsync(input);
        toast.success(t("Proxy created"));
        navigate("/admin/network/proxies", { replace: true });
      } else {
        // On edit, omit blank credentials to keep current values.
        const input: ProxyInput = {
          name: parsed.data.name,
          proxy_url: parsed.data.proxy_url,
          no_proxy_hosts: parsed.data.no_proxy_hosts,
          enabled: parsed.data.enabled,
        };
        if (parsed.data.username !== null) input.username = parsed.data.username;
        if (parsed.data.password !== null) input.password = parsed.data.password;
        await update.mutateAsync({ input, ifMatch: etag });
        toast.success(t("Proxy updated"));
      }
    } catch (error) {
      if (error instanceof ApiError && error.isConflict) {
        toast.error(t("This proxy was changed elsewhere. Reloading."));
      } else {
        toast.error(controlPlaneMutationErrorMessage(error, t("Save failed")));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const fieldError = (path: string) => {
    const message = validation?.issues.find((issue) => issue.path.join(".") === path)?.message;
    return message ? t(message) : undefined;
  };

  return (
    <AdminDetailShell
      title={isNew ? t("New proxy") : state.name || t("Proxy")}
      description={t("An egress proxy shared by upstream clients.")}
      backPath="/admin/network/proxies"
      backLabel={t("Back to proxies")}
      isLoading={isLoading}
      error={error}
      hasData={isNew || Boolean(data)}
      detailCard={
        <>
          {!isNew && data ? (
            <Card>
              <CardHeader>
                <CardTitle>{data.data.name}</CardTitle>
                <CardDescription className="font-mono">{data.data.proxy_url}</CardDescription>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                  <DetailField
                    label={t("Enabled")}
                    value={<StatusBadge value={data.data.enabled} />}
                  />
                  <DetailField
                    label={t("Credential configured")}
                    value={data.data.credential_configured ? t("yes") : t("no")}
                  />
                </dl>
              </CardContent>
            </Card>
          ) : null}
          {testResult ? <ProxyTestResultCard result={testResult} /> : null}
        </>
      }
      editCard={
        <Card>
          <CardHeader>
            <CardTitle>{isNew ? t("Create proxy") : t("Edit proxy")}</CardTitle>
            <CardDescription>
              {!isNew ? t("Leave credential fields blank to keep current values.") : null}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-4">
              <FieldGroup className="grid gap-5 xl:grid-cols-2">
                <Field data-invalid={Boolean(fieldError("name"))}>
                  <FieldLabel htmlFor="name">{t("Name")}</FieldLabel>
                  <Input
                    id="name"
                    value={state.name}
                    onChange={(event) => patch({ name: event.target.value })}
                    aria-invalid={Boolean(fieldError("name"))}
                  />
                  {fieldError("name") ? <FieldError>{fieldError("name")}</FieldError> : null}
                </Field>
                <Field data-invalid={Boolean(fieldError("proxy_url"))}>
                  <FieldLabel htmlFor="proxy_url">{t("Proxy URL")}</FieldLabel>
                  <Input
                    id="proxy_url"
                    value={state.proxy_url}
                    onChange={(event) => patch({ proxy_url: event.target.value })}
                    placeholder="https://proxy.example:1080"
                    aria-invalid={Boolean(fieldError("proxy_url"))}
                  />
                  {fieldError("proxy_url") ? <FieldError>{fieldError("proxy_url")}</FieldError> : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="username">{t("Username")}</FieldLabel>
                  <Input
                    id="username"
                    value={state.username ?? ""}
                    onChange={(event) => patch({ username: event.target.value || null })}
                    autoComplete="off"
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="password">{t("Password")}</FieldLabel>
                  <Input
                    id="password"
                    type="password"
                    value={state.password ?? ""}
                    onChange={(event) => patch({ password: event.target.value || null })}
                    autoComplete="new-password"
                  />
                </Field>
                <StringListField
                  label={t("No-proxy hosts")}
                  description={t("Hosts that bypass the proxy.")}
                  value={state.no_proxy_hosts}
                  onChange={(value) => patch({ no_proxy_hosts: value })}
                  placeholder="example.com, .internal"
                />
                <Field orientation="horizontal">
                  <FieldLabel htmlFor="proxy_enabled">{t("Enabled")}</FieldLabel>
                  <Switch
                    id="proxy_enabled"
                    checked={state.enabled}
                    onCheckedChange={(checked) => patch({ enabled: Boolean(checked) })}
                  />
                </Field>
              </FieldGroup>
              <Alert>
                <AlertTitle>{t("ip-api.com diagnostic")}</AlertTitle>
                <AlertDescription>
                  {t(
                    "The test sends a fixed IP lookup through this proxy. The free ip-api.com endpoint is HTTP-only and restricted to non-commercial use; treat the result as diagnostic information.",
                  )}
                </AlertDescription>
              </Alert>
              <div className="flex flex-wrap gap-2 self-start">
                <Button
                  onClick={submit}
                  disabled={submitting || testProxy.isPending}
                >
                  {submitting ? <Spinner data-icon="inline-start" /> : null}
                  {isNew ? t("Create proxy") : t("Save proxy")}
                </Button>
                <Button
                  variant="outline"
                  onClick={runTest}
                  disabled={submitting || testProxy.isPending}
                >
                  {testProxy.isPending ? <Spinner data-icon="inline-start" /> : null}
                  {t("Test proxy")}
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      }
    />
  );
}

function ProxyTestResultCard({ result }: { result: ProxyTestResponse }) {
  const { t } = useI18n();
  const location =
    joinPresent([
      result.city,
      result.district,
      result.region_name,
      result.country_code ? `${result.country ?? ""} (${result.country_code})`.trim() : result.country,
    ]) || "—";
  const coordinates =
    result.latitude !== null && result.longitude !== null
      ? `${result.latitude}, ${result.longitude}`
      : "—";
  const timezone = joinPresent([
    result.timezone,
    formatUtcOffset(result.utc_offset_seconds),
  ]) || "—";
  const autonomousSystem =
    joinPresent([result.autonomous_system, result.autonomous_system_name]) || "—";
  const providerQuota =
    result.rate_limit_remaining === null
      ? "—"
      : result.rate_limit_reset_seconds === null
        ? String(result.rate_limit_remaining)
        : t("{remaining} remaining; reset in {seconds}s", {
            remaining: result.rate_limit_remaining,
            seconds: result.rate_limit_reset_seconds,
          });

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Proxy test result")}</CardTitle>
        <CardDescription>
          {t("Observed through ip-api.com in {duration}.", {
            duration: formatDurationMs(result.latency_ms),
          })}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <DetailField label={t("IP address")} value={result.ip} mono />
          <DetailField label={t("Location")} value={location} />
          <DetailField label={t("Coordinates")} value={coordinates} mono />
          <DetailField label={t("Timezone")} value={timezone} />
          <DetailField label={t("ISP")} value={result.isp ?? "—"} />
          <DetailField label={t("Organization")} value={result.organization ?? "—"} />
          <DetailField label={t("Autonomous system")} value={autonomousSystem} />
          <DetailField label={t("Mobile network")} value={formatBoolean(result.mobile)} />
          <DetailField label={t("Proxy detected")} value={formatBoolean(result.proxy)} />
          <DetailField label={t("Hosting network")} value={formatBoolean(result.hosting)} />
          <DetailField label={t("Provider quota")} value={providerQuota} />
        </dl>
      </CardContent>
    </Card>
  );
}

function joinPresent(values: Array<string | null>): string {
  return values.filter((value): value is string => Boolean(value)).join(", ");
}

function formatUtcOffset(seconds: number | null): string | null {
  if (seconds === null) return null;
  const sign = seconds < 0 ? "-" : "+";
  const absolute = Math.abs(seconds);
  const hours = Math.floor(absolute / 3_600);
  const minutes = Math.floor((absolute % 3_600) / 60);
  return `UTC${sign}${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}`;
}
