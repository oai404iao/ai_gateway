export function resourceLabels(rows: readonly { id: string; name: string }[]): Map<string, string> {
  const groups = new Map<string, Set<string>>();
  for (const { id, name } of rows) {
    const ids = groups.get(name) ?? new Set<string>();
    ids.add(id);
    groups.set(name, ids);
  }

  const labels = new Map<string, string>();
  for (const [name, unique] of groups) {
    const ids = [...unique];
    let length = 6;
    while (new Set(ids.map(id => id.slice(-length))).size < ids.length) length++;
    for (const id of ids) labels.set(id, ids.length === 1 ? name : `${name} · ${id.slice(-length)}`);
  }
  return labels;
}
