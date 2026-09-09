import type { CodexSharingUsage } from "@/api/types";
import { useI18n } from "@/app/i18n";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export function SharingUsage({ usage }: { usage: CodexSharingUsage }) {
  const { t, locale } = useI18n();
  return <div className="flex flex-col gap-3">
    <div className="flex flex-wrap gap-2">
      <Badge variant="secondary">{t(usage.available ? "Ledger ready" : "Waiting for sharing window")}</Badge>
      <Badge variant="outline">{t("Pending requests")}: {usage.pending_requests}</Badge>
    </div>
    {usage.uncertain && <Alert><AlertDescription>
      {t("An earlier request has unknown usage. Its reservation is retained until reconciliation or all affected windows reset.")}
    </AlertDescription></Alert>}
    {usage.windows.length > 0 && <div className="grid grid-cols-2 gap-3 sm:hidden">
      {usage.windows.map(window => <div key={window.window_id} className="min-w-0 rounded-lg border p-3">
        <p className="text-xs text-muted-foreground">{t(window.window_kind === "primary" ? "Primary window" : "Secondary window")}</p>
        <p className="mt-1 break-words text-sm font-semibold">{t("Remaining")}: ${window.remaining_amount}</p>
        <p className="mt-1 break-words text-xs text-muted-foreground">{t("Car remaining")}: ${window.group_remaining_amount}</p>
      </div>)}
    </div>}
    {usage.windows.length > 0 && <Table>
      <TableHeader><TableRow>
        {["Window", "Seat allowance", "Used / inherited", "Reserved", "Remaining", "Car remaining", "Provider used", "Reset time", "Last checked"].map(label =>
          <TableHead key={label}>{t(label)}</TableHead>)}
      </TableRow></TableHeader>
      <TableBody>{usage.windows.map(window => <TableRow key={window.window_id}>
        <TableCell>{t(window.window_kind === "primary" ? "Primary window" : "Secondary window")}</TableCell>
        <TableCell>${window.limit_amount}</TableCell>
        <TableCell>${window.used_amount}</TableCell>
        <TableCell>${window.reserved_amount}</TableCell>
        <TableCell>${window.remaining_amount}</TableCell>
        <TableCell>${window.group_remaining_amount}</TableCell>
        <TableCell>{window.provider_used_percent}%</TableCell>
        <TableCell>{new Date(window.reset_at).toLocaleString(locale)}</TableCell>
        <TableCell>{new Date(window.checked_at).toLocaleString(locale)}</TableCell>
      </TableRow>)}</TableBody>
    </Table>}
  </div>;
}
