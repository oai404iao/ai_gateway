export function accountInitials(name: string): string {
  const parts = name.trim().split(/\s+/);
  const letters =
    parts.length > 1
      ? parts.slice(0, 2).map((part) => Array.from(part)[0])
      : Array.from(parts[0]).slice(0, 2);
  return letters.join("").toUpperCase() || "?";
}
