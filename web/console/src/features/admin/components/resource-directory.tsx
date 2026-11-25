import { useId, useMemo, useRef, useState, type ReactNode } from "react";
import { useSearchParams } from "react-router";
import { ArrowUpRight, ChevronLeft, ChevronRight, PanelRightOpen, Search, X } from "lucide-react";
import { useI18n } from "@/app/i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NavigationLink } from "@/components/shared/navigation-link";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { useListPagination } from "@/lib/use-list-pagination";
import { resourceLabels } from "@/lib/resource-labels";
import { cn } from "@/lib/utils";

interface Props<T> {
  rows: T[];
  columns: Column<T>[];
  rowKey: (row: T) => string;
  rowName: (row: T) => string;
  rowHref?: (row: T) => string;
  searchText?: (row: T) => string;
  linkColumnKey?: string;
  selectionColumnKey?: string;
  groupBy?: (row: T) => string;
  renderToolbar?: (pageRows: T[], matches: T[]) => ReactNode;
  renderProperties?: (row: T) => ReactNode;
}

export function ResourceDirectory<T>({ rows, columns, rowKey, rowName, rowHref, searchText = rowName,
  linkColumnKey = columns[0]?.key, selectionColumnKey, groupBy, renderToolbar, renderProperties }: Props<T>) {
  const { t } = useI18n();
  const [params, setParams] = useSearchParams();
  const search = params.get("q") ?? "";
  const pagination = useListPagination();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const inspector = useRef<HTMLElement>(null);
  const inspectorHeading = useId();
  const labels = useMemo(() => resourceLabels(rows.map((row) => ({ id: rowKey(row), name: rowName(row) }))), [rows, rowKey, rowName]);
  const needle = search.trim().toLocaleLowerCase();
  const matches = rows.filter((row) => !needle || `${searchText(row)} ${rowKey(row)}`.toLocaleLowerCase().includes(needle));
  const position = matches.findIndex((row) => rowKey(row) === selectedId);
  const selected = matches[position];
  const inspect = (row: T) => {
    setSelectedId(rowKey(row));
    requestAnimationFrame(() => inspector.current?.focus());
  };
  const close = () => {
    const returnId = `${inspectorHeading}-${selectedId}`;
    setSelectedId(null);
    requestAnimationFrame(() => (document.getElementById(returnId) ?? searchInput.current)?.focus());
  };
  const move = (step: number) => {
    const index = position + step;
    if (!matches[index]) return;
    const page = Math.floor(index / pagination.pageSize) + 1;
    if (page !== pagination.page) pagination.onPageChange(page);
    setSelectedId(rowKey(matches[index]));
  };
  const changeSearch = (value: string) => {
    setSelectedId(null);
    setParams((current) => {
      const next = new URLSearchParams(current);
      if (value) next.set("q", value); else next.delete("q");
      next.delete("page");
      return next;
    }, { replace: true });
  };
  return <section className="@container min-w-0 border-y">
    <div className="flex flex-wrap items-center justify-between gap-2 border-b py-1.5">
      <label className="relative min-w-0"><Search aria-hidden className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input ref={searchInput} type="search" value={search} aria-label={t("Search records")} placeholder={t("Search records")} className="h-8 w-52 max-w-full pl-7" onChange={(event) => changeSearch(event.target.value)} />
      </label>
      <span className="text-xs text-muted-foreground">{t("{matches} of {total}", { matches: matches.length, total: rows.length })}</span>
    </div>
    <div className={cn("grid min-w-0", selected && "@min-[48rem]:grid-cols-[minmax(0,1fr)_minmax(16rem,.46fr)]")}>
      <div className={cn("min-w-0 py-2 [&_tr:has([aria-pressed=true])]:bg-accent", selected && "hidden @min-[48rem]:block")}>
        <ResourceTable rows={matches} rowKey={rowKey} rowHref={rowHref} linkColumnKey={linkColumnKey} groupBy={groupBy} pagination={pagination}
          renderToolbar={renderToolbar ? (items) => renderToolbar(items, matches) : undefined}
          columns={[...columns, { key: "inspect", header: "", pinned: true, basis: 2.75, className: "w-11", render: (row) => (
            <Button id={`${inspectorHeading}-${rowKey(row)}`} variant="ghost" size="icon-sm" aria-label={t("Inspect {name}", { name: labels.get(rowKey(row))! })} aria-pressed={rowKey(row) === selectedId} onClick={() => inspect(row)}>
              <PanelRightOpen data-icon="inline-start" />
            </Button>
          ) }]} onRowClick={inspect}
          empty={<p className="py-8 text-center text-sm text-muted-foreground">{t("No matching records")}</p>} />
      </div>
      {selected && <aside ref={inspector} tabIndex={-1} aria-labelledby={inspectorHeading} className="min-w-0 bg-muted/25 p-3 focus-visible:outline-2 focus-visible:outline-ring @min-[48rem]:border-l">
        <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
          <span>{t("Record properties")}</span>
          <div className="flex items-center gap-1">
            <Button variant="ghost" size="icon-sm" aria-label={t("Previous record")} disabled={position === 0} onClick={() => move(-1)}><ChevronLeft /></Button>
            <span>{position + 1} / {matches.length}</span>
            <Button variant="ghost" size="icon-sm" aria-label={t("Next record")} disabled={position + 1 === matches.length} onClick={() => move(1)}><ChevronRight /></Button>
            <Button variant="ghost" size="icon-sm" aria-label={t("Close properties")} onClick={close}><X /></Button>
          </div>
        </div>
        <h3 id={inspectorHeading} className="mt-2 break-all font-semibold">{labels.get(rowKey(selected))}</h3>
        {renderProperties ? renderProperties(selected) : <dl className="my-3 grid gap-2 border-y py-3 text-sm">{columns.filter((c) => c.key !== selectionColumnKey && c.key !== linkColumnKey).map((col) =>
          <div key={col.key} className="grid grid-cols-[minmax(0,.9fr)_minmax(0,1.1fr)] gap-2"><dt className="text-xs text-muted-foreground">{col.header}</dt><dd className="wrap-anywhere">{col.render(selected)}</dd></div>)}</dl>}
        {rowHref && <NavigationLink to={rowHref(selected)}>{t("Open configuration")}<ArrowUpRight data-icon="inline-end" /></NavigationLink>}
      </aside>}
    </div>
  </section>;
}
