import { useI18n } from "@/app/i18n";

export function CodexAccountIdentity({ accountId, userId }: { accountId: string | null; userId: string | null }) {
  const { t } = useI18n();
  const label = t(accountId ? "Workspace account" : "Personal Codex account");
  return <div className="mt-1 text-xs text-muted-foreground">
    {accountId || userId ? <details>
      <summary className="cursor-pointer">{label}</summary>
      {accountId && <p className="break-all">{t("Workspace {id}", { id: accountId })}</p>}
      {userId && <p className="break-all">{t(accountId ? "Member {id}" : "User {id}", { id: userId })}</p>}
    </details> : label}
  </div>;
}
