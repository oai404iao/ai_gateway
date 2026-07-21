export function maskApiKey(value: string): string {
  const prefix = value.startsWith("sk-") ? "sk-" : "";
  return `${prefix}${"•".repeat(24)}`;
}
