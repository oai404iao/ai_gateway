import { lazy } from "react";
import { Link, Navigate, Outlet, Route, Routes } from "react-router";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/shared/empty-state";
import { useI18n } from "@/app/i18n";
import { useSession } from "@/lib/use-session";
import { ConsoleLayout } from "@/app/layouts/console-layout";
import { AuthLayout } from "@/app/layouts/auth-layout";

// Page components are lazy-loaded so each route ships in its own chunk.
// The shell (providers, layouts, sidebar) stays eager for instant first paint.
const LoginPage = lazy(() =>
  import("@/features/auth/login-page").then((m) => ({ default: m.LoginPage })),
);
const RegisterPage = lazy(() =>
  import("@/features/auth/register-page").then((m) => ({ default: m.RegisterPage })),
);
const ActivateInvitationPage = lazy(() =>
  import("@/features/auth/activate-invitation-page").then((m) => ({
    default: m.ActivateInvitationPage,
  })),
);
const CompletePasswordResetPage = lazy(() =>
  import("@/features/auth/complete-password-reset-page").then((m) => ({
    default: m.CompletePasswordResetPage,
  })),
);
const ProfilePage = lazy(() =>
  import("@/features/profile/profile-page").then((m) => ({ default: m.ProfilePage })),
);
const PersonalSettingsPage = lazy(() =>
  import("@/features/personal-settings/personal-settings-page").then((m) => ({
    default: m.PersonalSettingsPage,
  })),
);
const SessionsPage = lazy(() =>
  import("@/features/sessions/sessions-page").then((m) => ({ default: m.SessionsPage })),
);
const ApiKeysPage = lazy(() =>
  import("@/features/api-keys/api-keys-page").then((m) => ({ default: m.ApiKeysPage })),
);
const ApiKeyDetailPage = lazy(() =>
  import("@/features/api-keys/api-key-detail-page").then((m) => ({
    default: m.ApiKeyDetailPage,
  })),
);
const OwnRequestLogsPage = lazy(() =>
  import("@/features/request-logs/own-request-logs-page").then((m) => ({
    default: m.OwnRequestLogsPage,
  })),
);
const CodexQuotasPage = lazy(() =>
  import("@/features/codex-quotas/codex-quotas-page").then((m) => ({
    default: m.CodexQuotasPage,
  })),
);
const AdminRequestLogsPage = lazy(() =>
  import("@/features/request-logs/admin-request-logs-page").then((m) => ({
    default: m.AdminRequestLogsPage,
  })),
);
const StatisticsPage = lazy(() =>
  import("@/features/statistics/statistics-page").then((m) => ({
    default: m.StatisticsPage,
  })),
);
const SystemCostStatisticsPage = lazy(() =>
  import("@/features/statistics/system-cost-statistics-page").then((m) => ({
    default: m.SystemCostStatisticsPage,
  })),
);
const ChannelGroupStatusPage = lazy(() =>
  import("@/features/statistics/channel-group-status-page").then((m) => ({
    default: m.ChannelGroupStatusPage,
  })),
);
const SpendLeaderboardPage = lazy(() =>
  import("@/features/spend-leaderboard/spend-leaderboard-page").then((m) => ({
    default: m.SpendLeaderboardPage,
  })),
);
const UsersPage = lazy(() =>
  import("@/features/admin/users/users-page").then((m) => ({ default: m.UsersPage })),
);
const UserDetailPage = lazy(() =>
  import("@/features/admin/users/user-detail-page").then((m) => ({
    default: m.UserDetailPage,
  })),
);
const UserGroupsPage = lazy(() =>
  import("@/features/admin/user-groups/user-groups-page").then((m) => ({
    default: m.UserGroupsPage,
  })),
);
const UserGroupDetailPage = lazy(() =>
  import("@/features/admin/user-groups/user-group-detail-page").then((m) => ({
    default: m.UserGroupDetailPage,
  })),
);
const RegistrationInvitationCodesPage = lazy(() =>
  import(
    "@/features/admin/registration-invitation-codes/registration-invitation-codes-page"
  ).then((m) => ({ default: m.RegistrationInvitationCodesPage })),
);
const RegistrationInvitationCodeDetailPage = lazy(() =>
  import(
    "@/features/admin/registration-invitation-codes/registration-invitation-code-detail-page"
  ).then((m) => ({ default: m.RegistrationInvitationCodeDetailPage })),
);
const ApiKeyPoliciesPage = lazy(() =>
  import("@/features/admin/api-key-policies/api-key-policies-page").then((m) => ({
    default: m.ApiKeyPoliciesPage,
  })),
);
const ApiKeyPolicyDetailPage = lazy(() =>
  import("@/features/admin/api-key-policies/policy-detail-page").then((m) => ({
    default: m.ApiKeyPolicyDetailPage,
  })),
);
const ModelsPage = lazy(() =>
  import("@/features/admin/models/models-page").then((m) => ({ default: m.ModelsPage })),
);
const ModelDetailPage = lazy(() =>
  import("@/features/admin/models/model-detail-page").then((m) => ({
    default: m.ModelDetailPage,
  })),
);
const CatalogPage = lazy(() =>
  import("@/features/admin/catalog/catalog-page").then((m) => ({ default: m.CatalogPage })),
);
const ChannelGroupDetailPage = lazy(() =>
  import("@/features/admin/routing/channel-groups/channel-group-detail-page").then((m) => ({
    default: m.ChannelGroupDetailPage,
  })),
);
const ChannelsPage = lazy(() =>
  import("@/features/admin/routing/channels/channels-page").then((m) => ({
    default: m.ChannelsPage,
  })),
);
const ChannelDetailPage = lazy(() =>
  import("@/features/admin/routing/channels/channel-detail-page").then((m) => ({
    default: m.ChannelDetailPage,
  })),
);
const ModelRulesPage = lazy(() =>
  import("@/features/admin/routing/model-rules/model-rules-page").then((m) => ({
    default: m.ModelRulesPage,
  })),
);
const ModelRuleDetailPage = lazy(() =>
  import("@/features/admin/routing/model-rules/model-rule-detail-page").then((m) => ({
    default: m.ModelRuleDetailPage,
  })),
);
const McpServersPage = lazy(() =>
  import("@/features/admin/mcp-servers/mcp-servers-page").then((m) => ({
    default: m.McpServersPage,
  })),
);
const McpServerDetailPage = lazy(() =>
  import("@/features/admin/mcp-servers/mcp-server-detail-page").then((m) => ({
    default: m.McpServerDetailPage,
  })),
);
const CodexOauthPage = lazy(
  () => import("@/features/admin/providers/codex-oauth/codex-oauth-page"),
);
const CodexImportPage = lazy(
  () => import("@/features/admin/providers/codex-oauth/codex-import-page"),
);
const ProxiesPage = lazy(() =>
  import("@/features/admin/network/proxies-page").then((m) => ({ default: m.ProxiesPage })),
);
const ProxyDetailPage = lazy(() =>
  import("@/features/admin/network/proxy-detail-page").then((m) => ({
    default: m.ProxyDetailPage,
  })),
);
const ConfigTemplatesPage = lazy(() =>
  import("@/features/admin/transforms/templates-page").then((m) => ({
    default: m.ConfigTemplatesPage,
  })),
);
const ConfigTemplateDetailPage = lazy(() =>
  import("@/features/admin/transforms/template-detail-page").then((m) => ({
    default: m.ConfigTemplateDetailPage,
  })),
);
const AuditLogsPage = lazy(() =>
  import("@/features/admin/audit-logs/audit-logs-page").then((m) => ({
    default: m.AuditLogsPage,
  })),
);
const SystemLoadPage = lazy(() =>
  import("@/features/admin/system-load/system-load-page").then((m) => ({
    default: m.SystemLoadPage,
  })),
);
const SystemPage = lazy(() =>
  import("@/features/admin/system/system-page").then((m) => ({ default: m.SystemPage })),
);

function RequireAuth() {
  const { isAuthenticated } = useSession();
  if (!isAuthenticated) return <Navigate to="/login" replace />;
  return <Outlet />;
}

function RequirePasswordChange() {
  const { user } = useSession();
  if (!user?.password_change_required) return <Navigate to="/account" replace />;
  return <Outlet />;
}

function RequireFullSession() {
  const { user } = useSession();
  if (user?.password_change_required) {
    return <Navigate to="/change-password" replace />;
  }
  return <Outlet />;
}

function RequireAdmin() {
  const { user } = useSession();
  if (user?.role !== "admin") return <Navigate to="/account" replace />;
  return <Outlet />;
}

function NotFound() {
  const { t } = useI18n();
  return (
    <EmptyState
      title={t("Not found")}
      description={t("The page you were looking for does not exist.")}
      className="min-h-80 border"
      actions={
        <Button render={<Link to="/account" />} nativeButton={false}>
          {t("Back to account")}
        </Button>
      }
    />
  );
}

export function AppRouter() {
  return (
    <Routes>
      <Route element={<AuthLayout />}>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/register" element={<RegisterPage />} />
        <Route path="/activate-invitation" element={<ActivateInvitationPage />} />
      </Route>

      <Route element={<RequireAuth />}>
        <Route element={<RequirePasswordChange />}>
          <Route element={<AuthLayout />}>
            <Route path="/change-password" element={<CompletePasswordResetPage />} />
          </Route>
        </Route>

        <Route element={<RequireFullSession />}>
          <Route element={<ConsoleLayout />}>
            <Route index element={<Navigate to="/statistics" replace />} />
            <Route path="/account" element={<ProfilePage />} />
            <Route path="/account/settings" element={<PersonalSettingsPage />} />
            <Route path="/account/sessions" element={<SessionsPage />} />
            <Route path="/api-keys" element={<ApiKeysPage />} />
            <Route path="/api-keys/:id" element={<ApiKeyDetailPage />} />
            <Route path="/usage/request-logs" element={<OwnRequestLogsPage />} />
            <Route path="/codex-quotas" element={<CodexQuotasPage />} />
            <Route path="/channel-group-status" element={<ChannelGroupStatusPage />} />
            <Route path="/statistics" element={<StatisticsPage />} />
            <Route path="/leaderboard" element={<SpendLeaderboardPage />} />

            <Route element={<RequireAdmin />}>
              <Route path="/admin/users" element={<UsersPage />} />
              <Route path="/admin/users/:id" element={<UserDetailPage />} />
              <Route path="/admin/user-groups" element={<UserGroupsPage />} />
              <Route path="/admin/user-groups/:id" element={<UserGroupDetailPage />} />
              <Route
                path="/admin/registration-invitation-codes"
                element={<RegistrationInvitationCodesPage />}
              />
              <Route
                path="/admin/registration-invitation-codes/:id"
                element={<RegistrationInvitationCodeDetailPage />}
              />
              <Route path="/admin/api-key-policies" element={<ApiKeyPoliciesPage />} />
              {/* Detail pages use the "new" path segment as their create-mode sentinel. */}
              <Route
                path="/admin/api-key-policies/:id"
                element={<ApiKeyPolicyDetailPage />}
              />
              <Route path="/admin/models" element={<ModelsPage />} />
              <Route path="/admin/models/:id" element={<ModelDetailPage />} />
              <Route path="/admin/catalog" element={<CatalogPage />} />
              <Route
                path="/admin/routing/channel-groups/:id"
                element={<ChannelGroupDetailPage />}
              />
              <Route path="/admin/routing/channels" element={<ChannelsPage />} />
              <Route path="/admin/routing/channels/:id" element={<ChannelDetailPage />} />
              <Route path="/admin/routing/model-rules" element={<ModelRulesPage />} />
              <Route
                path="/admin/routing/model-rules/:id"
                element={<ModelRuleDetailPage />}
              />
              <Route path="/admin/mcp-servers" element={<McpServersPage />} />
              <Route
                path="/admin/mcp-servers/:id"
                element={<McpServerDetailPage />}
              />
              <Route
                path="/admin/providers/codex-oauth/:id"
                element={<CodexOauthPage />}
              />
              <Route
                path="/admin/providers/codex-oauth/:id/import"
                element={<CodexImportPage />}
              />
              <Route path="/admin/network/proxies" element={<ProxiesPage />} />
              <Route
                path="/admin/network/proxies/:id"
                element={<ProxyDetailPage />}
              />
              <Route
                path="/admin/transforms/templates"
                element={<ConfigTemplatesPage />}
              />
              <Route
                path="/admin/transforms/templates/:id"
                element={<ConfigTemplateDetailPage />}
              />
              <Route
                path="/admin/statistics"
                element={<Navigate to="/admin/cost-statistics" replace />}
              />
              <Route
                path="/admin/cost-statistics"
                element={<SystemCostStatisticsPage />}
              />
              <Route path="/admin/request-logs" element={<AdminRequestLogsPage />} />
              <Route path="/admin/audit-logs" element={<AuditLogsPage />} />
              <Route path="/admin/system-load" element={<SystemLoadPage />} />
              <Route path="/admin/system" element={<SystemPage />} />
            </Route>

            <Route path="*" element={<NotFound />} />
          </Route>
        </Route>
      </Route>
    </Routes>
  );
}
