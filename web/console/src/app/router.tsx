import { Navigate, Outlet, Route, Routes } from "react-router";
import { useSession } from "@/lib/use-session";
import { ConsoleLayout } from "@/app/layouts/console-layout";
import { AuthLayout } from "@/app/layouts/auth-layout";
import { LoginPage } from "@/features/auth/login-page";
import { ActivateInvitationPage } from "@/features/auth/activate-invitation-page";
import { ProfilePage } from "@/features/profile/profile-page";
import { SessionsPage } from "@/features/sessions/sessions-page";
import { ApiKeysPage } from "@/features/api-keys/api-keys-page";
import { ApiKeyDetailPage } from "@/features/api-keys/api-key-detail-page";
import { OwnRequestLogsPage } from "@/features/request-logs/own-request-logs-page";
import { AdminRequestLogsPage } from "@/features/request-logs/admin-request-logs-page";
import { UsersPage } from "@/features/admin/users/users-page";
import { UserDetailPage } from "@/features/admin/users/user-detail-page";
import { ApiKeyPoliciesPage } from "@/features/admin/api-key-policies/api-key-policies-page";
import { ApiKeyPolicyDetailPage } from "@/features/admin/api-key-policies/policy-detail-page";
import { ModelsPage } from "@/features/admin/models/models-page";
import { ModelDetailPage } from "@/features/admin/models/model-detail-page";
import { CatalogPage } from "@/features/admin/catalog/catalog-page";
import { ChannelGroupsPage } from "@/features/admin/routing/channel-groups/channel-groups-page";
import { ChannelGroupDetailPage } from "@/features/admin/routing/channel-groups/channel-group-detail-page";
import { ChannelsPage } from "@/features/admin/routing/channels/channels-page";
import { ChannelDetailPage } from "@/features/admin/routing/channels/channel-detail-page";
import { ModelRulesPage } from "@/features/admin/routing/model-rules/model-rules-page";
import { ModelRuleDetailPage } from "@/features/admin/routing/model-rules/model-rule-detail-page";
import { ProxiesPage } from "@/features/admin/network/proxies-page";
import { ProxyDetailPage } from "@/features/admin/network/proxy-detail-page";
import { ConfigTemplatesPage } from "@/features/admin/transforms/templates-page";
import { ConfigTemplateDetailPage } from "@/features/admin/transforms/template-detail-page";
import { AuditLogsPage } from "@/features/admin/audit-logs/audit-logs-page";
import { SystemPage } from "@/features/admin/system/system-page";

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
  return (
    <div className="flex flex-col gap-2">
      <h1 className="text-2xl font-semibold">Not found</h1>
      <p className="text-sm text-muted-foreground">
        The page you were looking for does not exist.
      </p>
    </div>
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
            <Route path="/admin/api-key-policies/new" element={<ApiKeyPolicyDetailPage />} />
            <Route path="/admin/api-key-policies/:id" element={<ApiKeyPolicyDetailPage />} />
            <Route path="/admin/models" element={<ModelsPage />} />
            <Route path="/admin/models/new" element={<ModelDetailPage />} />
            <Route path="/admin/models/:id" element={<ModelDetailPage />} />
            <Route path="/admin/catalog" element={<CatalogPage />} />
            <Route path="/admin/routing/channel-groups" element={<ChannelGroupsPage />} />
            <Route path="/admin/routing/channel-groups/new" element={<ChannelGroupDetailPage />} />
            <Route path="/admin/routing/channel-groups/:id" element={<ChannelGroupDetailPage />} />
            <Route path="/admin/routing/channels" element={<ChannelsPage />} />
            <Route path="/admin/routing/channels/new" element={<ChannelDetailPage />} />
            <Route path="/admin/routing/channels/:id" element={<ChannelDetailPage />} />
            <Route path="/admin/routing/model-rules" element={<ModelRulesPage />} />
            <Route path="/admin/routing/model-rules/new" element={<ModelRuleDetailPage />} />
            <Route path="/admin/routing/model-rules/:id" element={<ModelRuleDetailPage />} />
            <Route path="/admin/network/proxies" element={<ProxiesPage />} />
            <Route path="/admin/network/proxies/new" element={<ProxyDetailPage />} />
            <Route path="/admin/network/proxies/:id" element={<ProxyDetailPage />} />
            <Route path="/admin/transforms/templates" element={<ConfigTemplatesPage />} />
            <Route path="/admin/transforms/templates/new" element={<ConfigTemplateDetailPage />} />
            <Route path="/admin/transforms/templates/:id" element={<ConfigTemplateDetailPage />} />
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
