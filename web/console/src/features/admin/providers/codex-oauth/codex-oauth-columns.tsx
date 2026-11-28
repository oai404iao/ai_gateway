import { History, Pencil, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import type { CodexCredentialView } from "@/api/types";
import type { Column } from "@/components/shared/resource-table";
import { StatusBadge } from "@/components/shared/status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatDateTime } from "@/lib/dates";
import { CodexAccountIdentity } from "./codex-account-identity";
import { QuotaWindow } from "./codex-oauth-fields";
import {
  credentialStatusLabel,
  credentialStatusVariant,
  quotaSummary,
} from "./codex-oauth-helpers";

interface BuildCodexCredentialColumnsOptions {
  t: (key: string, values?: Record<string, string | number>) => string;
  selected: Set<string>;
  allSelected: boolean;
  proxies: Array<{ id: string; name: string }>;
  resetQuotaPending: boolean;
  refreshQuotaPending: boolean;
  refreshCredentialPending: boolean;
  deleteCredentialPending: boolean;
  onToggleAll: () => void;
  onToggleCredential: (credential: CodexCredentialView) => void;
  onOpenHistory: (credential: CodexCredentialView) => void;
  onEdit: (id: string) => void;
  onResetQuota: (credential: CodexCredentialView) => void;
  onRefresh: (kind: "quota" | "token", credential: CodexCredentialView) => void;
  onDelete: (credential: CodexCredentialView) => void;
}

export function buildCodexCredentialColumns({
  t,
  selected,
  allSelected,
  proxies,
  resetQuotaPending,
  refreshQuotaPending,
  refreshCredentialPending,
  deleteCredentialPending,
  onToggleAll,
  onToggleCredential,
  onOpenHistory,
  onEdit,
  onResetQuota,
  onRefresh,
  onDelete,
}: BuildCodexCredentialColumnsOptions): Column<CodexCredentialView>[] {
  return [
                {
                  key: "select",
                  header: (
                    <Checkbox
                      aria-label={t("Select all credentials")}
                      checked={allSelected}
                      onCheckedChange={onToggleAll}
                    />
                  ),
                  className: "w-12 align-top",
                  render: (credential) => (
                    <Checkbox
                      aria-label={t("Select {label}", {
                        label: credential.label,
                      })}
                      checked={selected.has(credential.id)}
                      onCheckedChange={() => onToggleCredential(credential)}
                    />
                  ),
                },
                {
                  key: "credential",
                  header: t("Credential"),
                  className: "min-w-52 align-top",
                  render: (credential) => (
                    <>
                      <div className="font-medium">{credential.label}</div>
                      {credential.email && credential.email !== credential.label && (
                        <div className="text-xs text-muted-foreground">{credential.email}</div>
                      )}
                      <CodexAccountIdentity accountId={credential.account_id} userId={credential.user_id} />
                      <div className="mt-2 flex flex-wrap gap-1">
                        {credential.plan_type ? (
                          <Badge variant="secondary">{credential.plan_type}</Badge>
                        ) : null}
                        {credential.is_fedramp ? (
                          <Badge variant="outline">FedRAMP</Badge>
                        ) : null}
                      </div>
                      <p className="mt-2 text-xs text-muted-foreground">
                        {t("Available models: {count}", {
                          count: credential.available_models.length,
                        })}
                      </p>
                      {credential.last_error_code ? (
                        <p className="mt-2 max-w-64 text-xs text-destructive">
                          {credential.last_error_code}
                          {credential.last_error_summary
                            ? `: ${credential.last_error_summary}`
                            : ""}
                        </p>
                      ) : null}
                    </>
                  ),
                },
                {
                  key: "status",
                  header: t("Status"),
                  className: "align-top",
                  render: (credential) => (
                    <>
                      <StatusBadge
                        value={credential.runtime_status}
                        label={credentialStatusLabel(credential.runtime_status)}
                        variant={credentialStatusVariant(
                          credential.runtime_status,
                        )}
                      />
                      {!credential.enabled ? (
                        <div className="mt-2">
                          <StatusBadge value={false} />
                        </div>
                      ) : null}
                    </>
                  ),
                },
                {
                  key: "quota",
                  header: t("Quota"),
                  className: "min-w-56 align-top",
                  render: (credential) => (
                    <>
                      <div className="mb-2 text-sm font-medium">
                        {quotaSummary(credential)}
                      </div>
                      <div className="space-y-3">
                        <QuotaWindow
                          label="Primary window"
                          percent={credential.primary_used_percent}
                          resetAt={credential.primary_reset_at}
                          costAmount={credential.primary_window_cost_amount}
                        />
                        {credential.secondary_used_percent !== null ? (
                          <QuotaWindow
                            label="Secondary window"
                            percent={credential.secondary_used_percent}
                            resetAt={credential.secondary_reset_at}
                            costAmount={credential.secondary_window_cost_amount}
                          />
                        ) : null}
                      </div>
                      <p className="mt-2 text-xs text-muted-foreground">
                        {t("Threshold: {percent}%", {
                          percent: credential.quota_threshold_percent,
                        })}
                      </p>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {t("OpenAI reset credits: {count}", {
                          count:
                            credential.quota_reset_credits_available ?? "—",
                        })}
                      </p>
                    </>
                  ),
                },
                {
                  key: "token",
                  header: t("Token"),
                  className: "min-w-44 align-top text-sm",
                  render: (credential) => (
                    <>
                      <p>{formatDateTime(credential.access_token_expires_at)}</p>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {t("Refreshed {time}", {
                          time: formatDateTime(credential.last_refreshed_at),
                        })}
                      </p>
                    </>
                  ),
                },
                {
                  key: "proxy",
                  header: t("Maintenance proxy"),
                  className: "align-top text-sm",
                  render: (credential) => (
                    <p className="text-xs text-muted-foreground">
                      {credential.proxy_id
                        ? proxies.find((proxy) => proxy.id === credential.proxy_id)?.name ?? t("Proxy assigned")
                        : t("Direct")}
                    </p>
                  ),
                },
                {
                  key: "actions",
                  header: t("Actions"),
                  pinned: true,
                  className: "align-top",
                  render: (credential) => (
                    <div className="flex flex-col items-end gap-1">
                      <div className="flex gap-1">
                        <Tooltip disableHoverablePopup>
                          <TooltipTrigger
                            render={
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={t("View quota history for {label}", {
                                  label: credential.label,
                                })}
                                onClick={() => onOpenHistory(credential)}
                              />
                            }
                          >
                            <History />
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t("View quota history for {label}", {
                              label: credential.label,
                            })}
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip disableHoverablePopup>
                          <TooltipTrigger
                            render={
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={t("Edit {label}", {
                                  label: credential.label,
                                })}
                                onClick={() => onEdit(credential.id)}
                              />
                            }
                          >
                            <Pencil />
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t("Edit {label}", {
                              label: credential.label,
                            })}
                          </TooltipContent>
                        </Tooltip>
                      </div>
                      <div className="flex gap-1">
                        <Tooltip disableHoverablePopup>
                          <TooltipTrigger
                            render={
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={t(
                                  "Reset quota with an OpenAI credit for {label}",
                                  { label: credential.label },
                                )}
                                disabled={
                                  resetQuotaPending ||
                                  credential.quota_reset_credits_available === 0
                                }
                                onClick={() => onResetQuota(credential)}
                              />
                            }
                          >
                            <RotateCcw />
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t(
                              "Reset quota with an OpenAI credit for {label}",
                              { label: credential.label },
                            )}
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip disableHoverablePopup>
                          <TooltipTrigger
                            render={
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={t("Refresh quota for {label}", {
                                  label: credential.label,
                                })}
                                disabled={refreshQuotaPending}
                                onClick={() => onRefresh("quota", credential)}
                              />
                            }
                          >
                            <RefreshCw className="text-info" />
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t("Refresh quota for {label}", {
                              label: credential.label,
                            })}
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip disableHoverablePopup>
                          <TooltipTrigger
                            render={
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={t("Refresh token for {label}", {
                                  label: credential.label,
                                })}
                                disabled={refreshCredentialPending}
                                onClick={() => onRefresh("token", credential)}
                              />
                            }
                          >
                            <RefreshCw />
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t("Refresh token for {label}", {
                              label: credential.label,
                            })}
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip disableHoverablePopup>
                          <TooltipTrigger
                            render={
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={t("Delete {label}", {
                                  label: credential.label,
                                })}
                                disabled={deleteCredentialPending}
                                onClick={() => onDelete(credential)}
                              />
                            }
                          >
                            <Trash2 />
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t("Delete {label}", {
                              label: credential.label,
                            })}
                          </TooltipContent>
                        </Tooltip>
                      </div>
                    </div>
                  ),
                },
  ];
}
