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
const ActivateInvitationPage = lazy(() =>
  import("@/features/auth/activate-invitation-page").then((m) => ({
    default: m.ActivateInvitationPage,
  })),
);
const ProfilePage = lazy(() =>
  import("@/features/profile/profile-page").then((m) => ({ default: m.ProfilePage })),
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
const UsersPage = lazy(() =>
  import("@/features/admin/users/users-page").then((m) => ({ default: m.UsersPage })),
);
const UserDetailPage = lazy(() =>
  import("@/features/admin/users/user-detail-page").then((m) => ({
    default: m.UserDetailPage,
  })),
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
const SystemPage = lazy(() =>
  import("@/features/admin/system/system-page").then((m) => ({ default: m.SystemPage })),
);

function RequireAuth() {
  const { isAuthenticated } = useSession();
  if (!isAuthenticated) return <Navigate to="/login" replace />;
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
        <Route path="/activate-invitation" element={<ActivateInvitationPage />} />
      </Route>

      <Route element={<RequireAuth />}>
        <Route element={<ConsoleLayout />}>
          <Route index element={<Navigate to="/account" replace />} />
          <Route path="/account" element={<ProfilePage />} />
          <Route path="/account/sessions" element={<SessionsPage />} />
          <Route path="/api-keys" element={<ApiKeysPage />} />
          <Route path="/api-keys/:id" element={<ApiKeyDetailPage />} />
          <Route path="/usage/request-logs" element={<OwnRequestLogsPage />} />

          <Route element={<RequireAdmin />}>
            <Route path="/admin/users" element={<UsersPage />} />
            <Route path="/admin/users/:id" element={<UserDetailPage />} />
            <Route path="/admin/api-key-policies" element={<ApiKeyPoliciesPage />} />
            {/* Detail pages use the "new" path segment as their create-mode sentinel. */}
            <Route path="/admin/api-key-policies/:id" element={<ApiKeyPolicyDetailPage />} />
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
            <Route path="/admin/routing/model-rules/:id" element={<ModelRuleDetailPage />} />
            <Route path="/admin/network/proxies" element={<ProxiesPage />} />
            <Route path="/admin/network/proxies/:id" element={<ProxyDetailPage />} />
            <Route path="/admin/transforms/templates" element={<ConfigTemplatesPage />} />
            <Route
              path="/admin/transforms/templates/:id"
              element={<ConfigTemplateDetailPage />}
            />
            <Route path="/admin/statistics" element={<StatisticsPage />} />
            <Route path="/admin/request-logs" element={<AdminRequestLogsPage />} />
            <Route path="/admin/audit-logs" element={<AuditLogsPage />} />
            <Route path="/admin/system" element={<SystemPage />} />
          </Route>

          <Route path="*" element={<NotFound />} />
        </Route>
      </Route>
    </Routes>
  );
}
