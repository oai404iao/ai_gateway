import { lazy, useState } from "react";
import {
  Navigate,
  Outlet,
  Route,
  Routes,
  RouterProvider,
  createBrowserRouter,
  createRoutesFromElements,
} from "react-router";
import { NavigationLink } from "@/components/shared/navigation-link";
import { EmptyState } from "@/components/shared/empty-state";
import { useI18n } from "@/app/i18n";
import { useSession } from "@/lib/use-session";
import { sessionScope } from "@/api/session-store";
import { ConsoleLayout } from "@/app/layouts/console-layout";
import { AuthLayout } from "@/app/layouts/auth-layout";
import { ModelWorkspaceLayout } from "@/features/admin/models/model-workspace-layout";
import { SETTINGS_SECTIONS } from "@/features/admin/system/settings-sections";

// Page components are lazy-loaded so each route ships in its own chunk.
// The shell (providers and navigation) stays eager for instant first paint.
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
const GatewayOverviewPage = lazy(() =>
  import("@/features/statistics/gateway-overview-page").then((m) => ({ default: m.GatewayOverviewPage })),
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
const ModelOnboardingPage = lazy(() =>
  import("@/features/admin/model-setup/model-onboarding-page").then((m) => ({ default: m.ModelOnboardingPage })),
);
const ModelDetailPage = lazy(() =>
  import("@/features/admin/models/model-detail-page").then((m) => ({
    default: m.ModelDetailPage,
  })),
);
const ModelPricingPage = lazy(() =>
  import("@/features/admin/models/model-pricing-page").then((m) => ({
    default: m.ModelPricingPage,
  })),
);
const CodexImportPage = lazy(
  () => import("@/features/admin/providers/codex-oauth/codex-import-page"),
);
const AccessesPage = lazy(() =>
  import("@/features/admin/routing/accesses/accesses-page").then((m) => ({ default: m.AccessesPage })),
);
const AccessDetailPage = lazy(() =>
  import("@/features/admin/routing/accesses/access-detail-page").then((m) => ({ default: m.AccessDetailPage })),
);
const GroupDetailPage = lazy(() =>
  import("@/features/admin/routing/groups/group-detail-page").then((m) => ({ default: m.GroupDetailPage })),
);
const ChannelConfigurationPage = lazy(() =>
  import("@/features/admin/routing/channel-configuration-page").then((m) => ({
    default: m.ChannelConfigurationPage,
  })),
);
const LogicalChannelDetailPage = lazy(() =>
  import("@/features/admin/routing/logical-channels/logical-channel-detail-page").then((m) => ({
    default: m.LogicalChannelDetailPage,
  })),
);
const CapabilitiesPage = lazy(() =>
  import("@/features/admin/routing/capabilities/capabilities-page").then((m) => ({
    default: m.CapabilitiesPage,
  })),
);
const CapabilityDetailPage = lazy(() =>
  import("@/features/admin/routing/capabilities/capability-detail-page").then((m) => ({
    default: m.CapabilityDetailPage,
  })),
);
const OperationRulesPage = lazy(() =>
  import("@/features/admin/routing/operation-rules/operation-rules-page").then((m) => ({
    default: m.OperationRulesPage,
  })),
);
const OperationRuleDetailPage = lazy(() =>
  import("@/features/admin/routing/operation-rules/operation-rule-detail-page").then((m) => ({
    default: m.OperationRuleDetailPage,
  })),
);
const CredentialsPage = lazy(() =>
  import("@/features/admin/routing/credentials/credentials-page").then((m) => ({ default: m.CredentialsPage })),
);
const CredentialDetailPage = lazy(() =>
  import("@/features/admin/routing/credentials/credential-detail-page").then((m) => ({ default: m.CredentialDetailPage })),
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
const PluginsPage = lazy(() =>
  import("@/features/admin/plugins/plugins-page").then((m) => ({ default: m.PluginsPage })),
);
const PluginDetailPage = lazy(() =>
  import("@/features/admin/plugins/plugin-detail-page").then((m) => ({ default: m.PluginDetailPage })),
);

function RequireAuth() {
  const { isAuthenticated, user } = useSession();
  if (!isAuthenticated) return <Navigate to="/login" replace />;
  return <Outlet key={sessionScope(user)} />;
}

function RequirePasswordChange() {
  const { user } = useSession();
  if (!user?.password_change_required) return <Navigate to="/" replace />;
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
  if (user?.role !== "admin") return <Navigate to="/" replace />;
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
        <NavigationLink to="/account">
          {t("Back to account")}
        </NavigationLink>
      }
    />
  );
}

function appRouteElements() {
  return (
    <>
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
            <Route index element={<GatewayOverviewPage />} />
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
              <Route path="/admin/routing/channels" element={<ChannelConfigurationPage />} />
              <Route path="/admin/models" element={<ModelWorkspaceLayout />}>
                <Route index element={<ModelsPage />} />
                <Route path="onboarding" element={<ModelOnboardingPage />} />
                <Route path=":id/onboarding" element={<ModelOnboardingPage />} />
                <Route path=":id" element={<ModelDetailPage />} />
                <Route path=":id/pricing" element={<ModelPricingPage />} />
              </Route>
              <Route path="/admin/routing/upstream-credentials" element={<CredentialsPage />} />
              <Route path="/admin/routing/accesses" element={<AccessesPage />} />
              <Route path="/admin/routing/groups/:id" element={<GroupDetailPage />} />
              <Route
                path="/admin/routing/logical-channels/:id"
                element={<LogicalChannelDetailPage />}
              />
              <Route path="/admin/routing/capabilities" element={<CapabilitiesPage />} />
              <Route
                path="/admin/routing/capabilities/:id"
                element={<CapabilityDetailPage />}
              />
              <Route
                path="/admin/routing/operation-rules"
                element={<OperationRulesPage />}
              />
              <Route
                path="/admin/routing/operation-rules/:id"
                element={<OperationRuleDetailPage />}
              />
              <Route path="/admin/routing/accesses/:id" element={<AccessDetailPage />} />
              <Route path="/admin/routing/upstream-credentials/:id" element={<CredentialDetailPage />} />
              <Route path="/admin/routing/upstream-credentials/codex/import" element={<CodexImportPage />} />
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
                path="/admin/cost-statistics"
                element={<SystemCostStatisticsPage />}
              />
              <Route path="/admin/request-logs" element={<AdminRequestLogsPage />} />
              <Route path="/admin/audit-logs" element={<AuditLogsPage />} />
              <Route path="/admin/system-load" element={<SystemLoadPage />} />
              {SETTINGS_SECTIONS.map(({ id, label }) => (
                <Route key={id} path={`/admin/system/${id}`} element={<SystemPage key={id} section={id} label={label} />} />
              ))}
              <Route path="/admin/plugins" element={<PluginsPage />} />
              <Route path="/admin/plugins/:id" element={<PluginDetailPage />} />
            </Route>

            <Route path="*" element={<NotFound />} />
          </Route>
        </Route>
      </Route>
    </>
  );
}

export function AppRouter() {
  return <Routes>{appRouteElements()}</Routes>;
}

export function AppRouterProvider() {
  const [router] = useState(() =>
    createBrowserRouter(createRoutesFromElements(appRouteElements())),
  );
  return <RouterProvider router={router} />;
}
