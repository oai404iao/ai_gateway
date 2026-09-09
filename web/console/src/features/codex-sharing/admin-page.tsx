import { Link } from "react-router";
import { useI18n } from "@/app/i18n";
import { AsyncResource } from "@/components/shared/async-resource";
import { PageHeader } from "@/components/shared/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useSharingGroups } from "./api";

export function SharingGroupsPage() {
  const query = useSharingGroups();
  const { t } = useI18n();
  return <div className="flex flex-col gap-6">
    <PageHeader title="Codex sharing" description="Dedicated credentials, fixed seats and provider-aligned USD windows."
      actions={<Button nativeButton={false} render={<Link to="/admin/codex-sharing/new" />}>{t("New sharing group")}</Button>} />
    {query.data && !query.data.runtime_available && <Alert><AlertDescription>
      {t("Enable codex_sharing.enabled on one gateway instance before enabling a sharing group.")}
    </AlertDescription></Alert>}
    <AsyncResource isLoading={query.isLoading} error={query.error} isEmpty={query.data?.groups.length === 0}>
      <Table><TableHeader><TableRow>
        {["Name", "Seats", "Primary window", "Secondary window", "Status"].map(label =>
          <TableHead key={label}>{t(label)}</TableHead>)}
      </TableRow></TableHeader><TableBody>
        {query.data?.groups.map(group => <TableRow key={group.id}>
          <TableCell><Link className="underline underline-offset-4" to={`/admin/codex-sharing/${group.id}`}>{group.name}</Link></TableCell>
          <TableCell>{group.seats.filter(Boolean).length} / {group.seats.length}</TableCell>
          <TableCell>${group.primary_limit_amount}</TableCell>
          <TableCell>${group.secondary_limit_amount}</TableCell>
          <TableCell>{t(group.enabled ? "Enabled" : "Paused")}</TableCell>
        </TableRow>)}
      </TableBody></Table>
    </AsyncResource>
  </div>;
}
