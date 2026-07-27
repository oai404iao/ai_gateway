import { useNavigate } from "react-router";
import type { RegistrationInvitationCodeView } from "@/api/types";
import { StatusBadge } from "@/components/shared/status-badge";
import { AdminListPage } from "@/features/admin/components/admin-list-page";
import {
  useRegistrationInvitationCodes,
  useUserGroups,
} from "@/features/admin/api";
import { useI18n } from "@/app/i18n";
import { formatExpiry, formatRelative } from "@/lib/dates";
import { formatUsd } from "@/lib/formatters";

type CodeStatus = "active" | "disabled" | "expired" | "exhausted";

function status(code: RegistrationInvitationCodeView): CodeStatus {
  if (!code.enabled) return "disabled";
  if (code.expires_at && new Date(code.expires_at).getTime() <= Date.now()) {
    return "expired";
  }
  if (code.max_uses !== null && code.used_count >= code.max_uses) {
    return "exhausted";
  }
  return "active";
}

export function RegistrationInvitationCodesPage() {
  const navigate = useNavigate();
  const codes = useRegistrationInvitationCodes();
  const groups = useUserGroups();
  const { t } = useI18n();

  const groupName = (id: string) =>
    groups.data?.find((group) => group.id === id)?.name ?? id;
  const statusBadge = (code: RegistrationInvitationCodeView) => {
    const value = status(code);
    if (value === "expired") {
      return <StatusBadge value={value} label={t("Expired")} variant="warning" />;
    }
    if (value === "exhausted") {
      return <StatusBadge value={value} label={t("Exhausted")} variant="warning" />;
    }
    return <StatusBadge value={value} />;
  };

  return (
    <AdminListPage
      title={t("Registration Codes")}
      description={t(
        "Create and adjust reusable invitation codes for self-service registration.",
      )}
      query={{
        data: codes.data,
        isLoading: codes.isLoading || groups.isLoading,
        error: codes.error ?? groups.error,
      }}
      rowKey={(code) => code.id}
      detailPath={(code) => `/admin/registration-invitation-codes/${code.id}`}
      createLabel={t("New registration code")}
      onCreate={() => navigate("/admin/registration-invitation-codes/new")}
      columns={[
        {
          key: "name",
          header: t("Name"),
          render: (code) => <span className="font-medium">{code.name}</span>,
        },
        {
          key: "status",
          header: t("Status"),
          render: statusBadge,
        },
        {
          key: "uses",
          header: t("Uses"),
          render: (code) =>
            code.max_uses === null
              ? t("{used} / unlimited", { used: code.used_count })
              : `${code.used_count} / ${code.max_uses}`,
        },
        {
          key: "group",
          header: t("User group"),
          render: (code) => groupName(code.user_group_id),
        },
        {
          key: "balance",
          header: t("Initial balance"),
          render: (code) => formatUsd(code.initial_balance_amount),
        },
        {
          key: "expires",
          header: t("Expires"),
          render: (code) => formatExpiry(code.expires_at),
        },
        {
          key: "updated",
          header: t("Updated"),
          render: (code) => formatRelative(code.updated_at),
        },
      ]}
    />
  );
}
