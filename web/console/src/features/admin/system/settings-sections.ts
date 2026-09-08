export const SETTINGS_SECTIONS = [
  { id: "general", label: "General settings" },
  { id: "upstream", label: "Upstream timeouts" },
  { id: "reliability", label: "Retries and health" },
  { id: "testing", label: "Scheduled channel tests" },
  { id: "affinity", label: "Session affinity" },
  { id: "websocket", label: "Responses WebSocket" },
  { id: "codex", label: "Codex" },
  { id: "maintenance", label: "Runtime maintenance" },
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number]["id"];
